#!/usr/bin/env python3
"""Pinned, fail-closed model/MCP harness for the Code Mode benchmark corpus."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
import platform
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
import grader


CORPUS = json.loads((ROOT / "corpus.json").read_text(encoding="utf-8"))
CONFIG = json.loads((ROOT / "harness-config.json").read_text(encoding="utf-8"))


class HarnessError(RuntimeError):
    pass


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_file(path: Path) -> str:
    return digest_bytes(path.read_bytes())


def run_git(arguments: list[str], cwd: Path | None = None) -> str:
    git_environment = {
        **os.environ,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }
    result = subprocess.run(
        ["git", *arguments], cwd=cwd, text=True, capture_output=True, check=False,
        env=git_environment,
    )
    if result.returncode:
        raise HarnessError(f"git {' '.join(arguments)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def replace_tree(source: Path, destination: Path) -> None:
    for child in destination.iterdir():
        if child.name != ".git":
            if child.is_dir():
                shutil.rmtree(child)
            else:
                child.unlink()
    for source_path in source.rglob("*"):
        relative = source_path.relative_to(source)
        target = destination / relative
        if source_path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_path, target)


def seed_repository(run_root: Path) -> tuple[Path, Path, dict[str, str]]:
    fixture_root = ROOT / CORPUS["repository"]["fixture_root"]
    worktree = run_root / "worktree"
    remote = run_root / "remote.git"
    cache = run_root / "cache"
    worktree.mkdir()
    run_git(["init", "--initial-branch=base", str(worktree)])
    replace_tree(fixture_root / "base", worktree)
    run_git(["add", "."], worktree)
    run_git(["-c", "user.name=wit-benchmark", "-c", "user.email=benchmark@example.invalid", "commit", "-m", "base"], worktree)
    base_sha = run_git(["rev-parse", "HEAD"], worktree)
    run_git(["checkout", "-b", "target"], worktree)
    replace_tree(fixture_root / "target", worktree)
    run_git(["add", "-A"], worktree)
    run_git(["-c", "user.name=wit-benchmark", "-c", "user.email=benchmark@example.invalid", "commit", "-m", "target"], worktree)
    target_sha = run_git(["rev-parse", "HEAD"], worktree)
    run_git(["init", "--bare", str(remote)])
    run_git(["remote", "add", "origin", str(remote)], worktree)
    run_git(["push", "origin", "base", "target"], worktree)
    run_git(["symbolic-ref", "HEAD", "refs/heads/base"], remote)
    git_config = run_root / "gitconfig"
    repository = CORPUS["repository"]["id"]
    git_config.write_text(
        f'[url "file://{remote}"]\n\tinsteadOf = https://github.com/{repository}\n'
        f'\tinsteadOf = https://github.com/{repository}.git\n',
        encoding="utf-8",
    )
    cache.mkdir()
    refs = {"base": base_sha, "target": target_sha}
    expected_refs = CORPUS["repository"].get("expected_commit_shas")
    if expected_refs is not None and refs != expected_refs:
        raise HarnessError(f"fixture commit SHAs differ from corpus: {refs}")
    return cache, git_config, refs


def benchmark_subprocess_environment(cache: Path, git_config: Path, metrics_path: Path) -> dict[str, str]:
    allowed = {
        "PATH", "TMPDIR", "LC_ALL", "TZ", "PYTHONHASHSEED", "RUST_LOG",
        "DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH", "SYSTEMROOT", "WINDIR",
        "COMSPEC", "PATHEXT",
    }
    environment = {key: value for key, value in os.environ.items() if key in allowed}
    environment.update(CONFIG["network"]["mcp_proxy_environment"])
    environment.update({
        "WIT_CACHE_DIR": str(cache),
        "GIT_CONFIG_GLOBAL": str(git_config),
        "GIT_CONFIG_NOSYSTEM": "1",
        CONFIG["instrumentation"]["environment_variable"]: str(metrics_path),
    })
    return environment


class McpClient:
    def __init__(self, command: list[str], environment: dict[str, str], stderr_path: Path):
        started = time.perf_counter_ns()
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr_path.open("wb"),
            text=True, bufsize=1, env=environment,
        )
        self.started_ns = started
        self.next_id = 1
        self.pending: dict[int, queue.Queue[dict[str, Any]]] = {}
        self.output_error: Exception | None = None
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        try:
            for line in self.process.stdout:
                message = json.loads(line)
                response_id = message.get("id")
                if isinstance(response_id, int) and response_id in self.pending:
                    self.pending[response_id].put(message)
        except Exception as error:  # captured and surfaced to the controlling thread
            self.output_error = error

    def send(self, message: dict[str, Any]) -> None:
        if self.process.poll() is not None:
            raise HarnessError(f"MCP process exited with {self.process.returncode}")
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def request(self, method: str, params: dict[str, Any], cancel_after_ms: int | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        response_queue: queue.Queue[dict[str, Any]] = queue.Queue(maxsize=1)
        self.pending[request_id] = response_queue
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        if cancel_after_ms is not None:
            time.sleep(cancel_after_ms / 1000)
            self.send({
                "jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": {"requestId": request_id, "reason": "benchmark cancellation"},
            })
        try:
            response = response_queue.get(timeout=CONFIG["responses_api"]["timeout_seconds"])
        except queue.Empty as error:
            raise HarnessError(f"MCP request timed out: {method}") from error
        finally:
            self.pending.pop(request_id, None)
        if "error" in response:
            return {"isError": True, "jsonrpcError": response["error"]}
        return response.get("result", {})

    def initialize(self) -> tuple[list[dict[str, Any]], float]:
        result = self.request("initialize", {
            "protocolVersion": CONFIG["mcp"]["protocol_version"],
            "capabilities": {},
            "clientInfo": {"name": CONFIG["mcp"]["client_name"], "version": CONFIG["mcp"]["client_version"]},
        })
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
        tools = self.request("tools/list", {}).get("tools")
        if not isinstance(tools, list):
            raise HarnessError("MCP tools/list did not return tools")
        startup_ms = (time.perf_counter_ns() - self.started_ns) / 1_000_000
        return tools, startup_ms

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=2)


class ResourceSampler:
    def __init__(self, root_pid: int):
        self.root_pid = root_pid
        self.stop_event = threading.Event()
        self.peak_rss = 0
        self.peak_cpu = 0.0
        self.peak_count = 1
        self.thread = threading.Thread(target=self._sample_loop, daemon=True)

    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self.stop_event.set()
        self.thread.join(timeout=2)

    def _sample_loop(self) -> None:
        interval = CONFIG["instrumentation"]["resource_sample_interval_ms"] / 1000
        while not self.stop_event.is_set():
            result = subprocess.run(
                ["ps", "-axo", "pid=,ppid=,rss=,%cpu="], text=True,
                capture_output=True, check=False,
            )
            rows = []
            for line in result.stdout.splitlines():
                fields = line.split()
                if len(fields) == 4:
                    try:
                        rows.append(tuple(map(float, fields)))
                    except ValueError:
                        pass
            descendants = {self.root_pid}
            changed = True
            while changed:
                changed = False
                for pid, ppid, _, _ in rows:
                    if int(ppid) in descendants and int(pid) not in descendants:
                        descendants.add(int(pid))
                        changed = True
            selected = [row for row in rows if int(row[0]) in descendants]
            if selected:
                self.peak_rss = max(self.peak_rss, int(sum(row[2] for row in selected) * 1024))
                self.peak_cpu = max(self.peak_cpu, sum(row[3] for row in selected))
                self.peak_count = max(self.peak_count, len(selected))
            self.stop_event.wait(interval)


def response_tools(mcp_tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
    converted = []
    for tool in mcp_tools:
        converted.append({
            "type": "function", "name": tool["name"],
            "description": tool.get("description", ""),
            "parameters": tool.get("inputSchema", {"type": "object", "properties": {}}),
            "strict": False,
        })
    return converted


def response_payload(
    task_input: list[dict[str, Any]], instructions: str, tools: list[dict[str, Any]],
    prompt_cache_key: str,
) -> dict[str, Any]:
    return {
        "model": CORPUS["policy"]["model"]["id"],
        "temperature": CORPUS["policy"]["model"]["temperature"],
        "instructions": instructions, "input": task_input, "tools": tools,
        "store": CONFIG["responses_api"]["store"],
        "parallel_tool_calls": CONFIG["responses_api"]["parallel_tool_calls"],
        "tool_choice": CONFIG["responses_api"]["tool_choice"],
        "truncation": CONFIG["responses_api"]["truncation"],
        "max_output_tokens": CONFIG["responses_api"]["max_output_tokens"],
        "prompt_cache_key": prompt_cache_key,
    }


def function_call_output(call_id: str, result: dict[str, Any]) -> tuple[dict[str, Any], int]:
    serialized = json.dumps(result, separators=(",", ":"), ensure_ascii=False)
    return {
        "type": "function_call_output", "call_id": call_id, "output": serialized,
    }, len(serialized.encode())


def post_response(api_key: str, payload: dict[str, Any]) -> tuple[dict[str, Any], int]:
    body = canonical_bytes(payload)
    request = urllib.request.Request(
        CONFIG["responses_api"]["endpoint"], data=body, method="POST",
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=CONFIG["responses_api"]["timeout_seconds"]) as response:
            raw = response.read()
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise HarnessError(f"Responses API returned HTTP {error.code}: {detail}") from error
    except urllib.error.URLError as error:
        raise HarnessError(f"Responses API request failed: {error}") from error
    parsed = json.loads(raw)
    if parsed.get("status") != "completed":
        raise HarnessError(f"Responses API did not complete: {parsed.get('status')}")
    return parsed, len(body)


def final_output_text(response: dict[str, Any]) -> str | None:
    parts = []
    for item in response.get("output", []):
        if item.get("type") == "message":
            parts.extend(part["text"] for part in item.get("content", []) if part.get("type") == "output_text")
    return "".join(parts) if parts else None


def package_version(name: str) -> str:
    try:
        return importlib.metadata.version(name)
    except importlib.metadata.PackageNotFoundError as error:
        raise HarnessError(f"required pinned package is missing: {name}") from error


def execute(task: dict[str, Any], mode: str, repetition: int, output: Path) -> None:
    os.environ.update(CONFIG["process_environment"])
    if hasattr(time, "tzset"):
        time.tzset()
    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key:
        raise HarnessError("OPENAI_API_KEY is unavailable; no evaluation record was written")
    if package_version("tiktoken") != CONFIG["tokenizer"]["package_version"]:
        raise HarnessError("installed tiktoken version differs from pinned harness version")
    if package_version("jsonschema") != CONFIG["schema_validator"]["package_version"]:
        raise HarnessError("installed jsonschema version differs from pinned harness version")
    import tiktoken

    command = [str((ROOT.parents[1] / part).resolve()) if index == 0 else part for index, part in enumerate(CONFIG["commands"][mode])]
    binary = Path(command[0])
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise HarnessError(f"pinned {mode} command is not executable: {binary}")

    with tempfile.TemporaryDirectory(prefix="wit-codemode-benchmark-") as directory:
        run_root = Path(directory)
        cache, git_config, fixture_refs = seed_repository(run_root)
        metrics_path = run_root / "mcp-metrics.json"
        stderr_path = run_root / "mcp.stderr"
        mcp_environment = benchmark_subprocess_environment(cache, git_config, metrics_path)
        client = McpClient(command, mcp_environment, stderr_path)
        sampler = ResourceSampler(client.process.pid)
        sampler.start()
        started_ns = time.perf_counter_ns()
        raw_responses = []
        outer_calls = invalid_calls = visible_input_bytes = visible_result_bytes = 0
        cached_input_tokens = 0
        try:
            mcp_tools, mcp_startup_ms = client.initialize()
            expected_names = 7 if mode == "direct" else 1
            if len(mcp_tools) != expected_names:
                raise HarnessError(f"{mode} tools/list returned {len(mcp_tools)}, expected {expected_names}")
            tools = response_tools(mcp_tools)
            tools_wire = canonical_bytes(tools)
            tokenizer = tiktoken.get_encoding(CONFIG["tokenizer"]["encoding"])
            input_items: list[dict[str, Any]] = [{"role": "user", "content": task["prompt"]}]
            system = CORPUS["policy"]["prompt"]["system"] + (
                f" Repository: {CORPUS['repository']['id']}. Return exactly one JSON object with "
                "status, error_code, and claims. Each claim has text and evidence; each evidence has "
                "ref, path, start_line, end_line, and quote. Do not wrap JSON in markdown."
            )
            final_text = None
            while outer_calls < CONFIG["mcp"]["maximum_outer_calls"]:
                payload = response_payload(
                    input_items, system, tools,
                    f"wit-codemode:{task['id']}:{mode}:{repetition}",
                )
                response, request_bytes = post_response(api_key, payload)
                visible_input_bytes += request_bytes
                raw_responses.append(response)
                if response.get("model") != CORPUS["policy"]["model"]["id"]:
                    raise HarnessError(f"Responses API returned unexpected model {response.get('model')}")
                cached_input_tokens += response.get("usage", {}).get("input_tokens_details", {}).get("cached_tokens", 0)
                input_items.extend(response.get("output", []))
                calls = [item for item in response.get("output", []) if item.get("type") == "function_call"]
                if not calls:
                    final_text = final_output_text(response)
                    break
                for call in calls:
                    outer_calls += 1
                    try:
                        arguments = json.loads(call["arguments"])
                    except (KeyError, json.JSONDecodeError):
                        invalid_calls += 1
                        arguments = {}
                    if call.get("name") not in {tool["name"] for tool in mcp_tools}:
                        invalid_calls += 1
                        result = {"isError": True, "message": "unknown tool"}
                    else:
                        cancel_ms = CONFIG["mcp"]["cancellation_after_ms"] if task["id"] == "cancelled-workflow" else None
                        result = client.request("tools/call", {"name": call["name"], "arguments": arguments}, cancel_ms)
                        if result.get("isError"):
                            invalid_calls += 1
                    output_item, output_bytes = function_call_output(call["call_id"], result)
                    visible_result_bytes += output_bytes
                    input_items.append(output_item)
            if final_text is None:
                raise HarnessError("model did not return final text within outer-call budget")
        finally:
            sampler.stop()
            client.close()
        wall_time_ms = (time.perf_counter_ns() - started_ns) / 1_000_000

        if not metrics_path.is_file():
            raise HarnessError("required MCP instrumentation sidecar was not produced")
        instrumentation = json.loads(metrics_path.read_text(encoding="utf-8"))
        for field in CONFIG["instrumentation"]["required_fields"]:
            if field not in instrumentation:
                raise HarnessError(f"MCP instrumentation is missing {field}")
        graded = grader.grade(task, final_text)
        raw = {"responses": raw_responses, "final_output_text": final_text}
        git_commit = run_git(["rev-parse", "HEAD"], ROOT.parents[1])
        record = {
            "schema_version": 2,
            "benchmark_kind": "model_evaluation",
            "run_id": f"{task['id']}-{mode}-{repetition}",
            "pair_id": f"{task['id']}-{repetition}",
            "corpus_id": CORPUS["corpus_id"],
            "corpus_sha256": digest_file(ROOT / "corpus.json"),
            "thresholds_sha256": digest_file(ROOT / "thresholds.json"),
            "implementation": {"git_commit": git_commit, "wit_mcp_sha256": digest_file(binary), "worker_sha256": None},
            "task_id": task["id"], "mode": mode, "repetition": repetition,
            "model": CORPUS["policy"]["model"], "cache": CORPUS["policy"]["cache"],
            "environment": {
                "platform": platform.platform(), "machine": platform.machine(),
                "python": platform.python_version(), "git": run_git(["--version"]),
                "tiktoken": package_version("tiktoken"), "jsonschema": package_version("jsonschema"),
                "locale": os.environ.get("LC_ALL") or os.environ.get("LANG", ""),
                "timezone": os.environ.get("TZ", "system"), "commands": CONFIG["commands"],
                "fixture_refs": fixture_refs, "mcp_startup_ms": mcp_startup_ms,
            },
            "status": graded["status"], "error_code": graded["error_code"],
            "metrics": {
                "tool_description_bytes": len(tools_wire),
                "tool_description_tokens": len(tokenizer.encode(tools_wire.decode())),
                "model_visible_input_bytes": visible_input_bytes,
                "model_visible_result_bytes": visible_result_bytes,
                "model_cached_input_tokens": cached_input_tokens,
                "outer_mcp_calls": outer_calls,
                "inner_host_calls": instrumentation["inner_host_calls"],
                "invalid_calls": invalid_calls,
                "wall_time_ms": wall_time_ms,
                "worker_startup_ms": instrumentation["worker_startup_ms"],
                "peak_rss_bytes": sampler.peak_rss,
                "peak_cpu_percent": sampler.peak_cpu,
                "peak_process_count": sampler.peak_count,
            },
            "raw": {**raw, "sha256": digest_bytes(canonical_bytes(raw))},
            "grading": {
                "grader_version": grader.GRADER_VERSION,
                "grader_sha256": digest_file(ROOT / "grader.py"),
                "claim_mapping": graded["claim_mapping"],
            },
            "response": graded["response"],
        }
        output.write_text(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--task", required=True)
    parser.add_argument("--mode", choices=("direct", "code"), required=True)
    parser.add_argument("--repetition", type=int, choices=range(1, 11), required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    task = next((item for item in CORPUS["tasks"] if item["id"] == args.task), None)
    if task is None:
        parser.error(f"unknown task: {args.task}")
    if args.output.exists():
        parser.error("output already exists; refusing to overwrite a run record")
    try:
        execute(task, args.mode, args.repetition, args.output)
    except HarnessError as error:
        print(f"benchmark harness failed closed: {error}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
