#!/usr/bin/env python3
"""Validate and score the fixed direct-MCP versus Code Mode corpus.

The runner intentionally does not invoke a model. Model adapters write one JSON
record per task/mode/repetition, and this program verifies policy equality,
fixture provenance, completeness, metrics, and the predeclared promotion gates.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
CORPUS_PATH = ROOT / "corpus.json"
THRESHOLDS_PATH = ROOT / "thresholds.json"

RECORD_KEYS = {
    "schema_version", "benchmark_kind", "run_id", "pair_id", "corpus_id",
    "corpus_sha256", "thresholds_sha256", "implementation", "task_id",
    "mode", "repetition", "model", "cache", "environment", "status",
    "error_code", "metrics", "raw", "grading", "response",
}
REQUIRED_RECORD_KEYS = RECORD_KEYS - {"error_code"}
METRIC_KEYS = {
    "tool_description_bytes", "tool_description_tokens",
    "model_visible_input_bytes", "model_visible_result_bytes",
    "model_cached_input_tokens",
    "outer_mcp_calls", "inner_host_calls", "invalid_calls", "wall_time_ms",
    "worker_startup_ms", "peak_rss_bytes", "peak_cpu_percent",
    "peak_process_count",
}
NONNEGATIVE_INTEGER_METRICS = {
    "tool_description_bytes", "tool_description_tokens",
    "model_visible_input_bytes", "model_visible_result_bytes",
    "model_cached_input_tokens",
    "outer_mcp_calls", "inner_host_calls", "invalid_calls", "peak_rss_bytes",
}


class ValidationError(ValueError):
    pass


def grade_model_output(task: dict[str, Any], raw_text: str) -> dict[str, Any]:
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    import grader
    return grader.grade(task, raw_text)


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValidationError(f"cannot read JSON {path}: {error}") from error


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validator_classes() -> tuple[Any, Any]:
    try:
        import importlib.metadata
        import jsonschema
    except ImportError as error:
        raise ValidationError(
            "pinned jsonschema dependency is missing; install requirements.lock"
        ) from error
    expected = load_json(ROOT / "harness-config.json")["schema_validator"]["package_version"]
    actual = importlib.metadata.version("jsonschema")
    if actual != expected:
        raise ValidationError(f"jsonschema version {actual} differs from pinned {expected}")
    return jsonschema.Draft202012Validator, jsonschema.exceptions.ValidationError


def validate_with_schema(instance: Any, schema_name: str) -> None:
    validator, validation_error = validator_classes()
    schema = load_json(ROOT / "schemas" / schema_name)
    try:
        validator(schema).validate(instance)
    except validation_error as error:
        path = "/".join(str(item) for item in error.absolute_path)
        raise ValidationError(f"{schema_name} rejected {path or '<root>'}: {error.message}") from error


def validate_package() -> tuple[dict[str, Any], dict[str, Any], str, str]:
    corpus = load_json(CORPUS_PATH)
    thresholds = load_json(THRESHOLDS_PATH)
    if corpus.get("schema_version") != 1 or thresholds.get("schema_version") != 1:
        raise ValidationError("corpus and thresholds schema_version must be 1")

    tasks = corpus.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise ValidationError("corpus tasks must be a non-empty array")
    task_ids = [task.get("id") for task in tasks]
    if len(task_ids) != len(set(task_ids)) or any(not item for item in task_ids):
        raise ValidationError("task ids must be non-empty and unique")
    classes = {task.get("class") for task in tasks}
    required_classes = {
        "simple_open_read", "symbol_search_precise_read",
        "multipage_search_filter", "multifile_context", "compare_refs",
        "failure_and_cancellation",
    }
    if classes != required_classes:
        raise ValidationError(
            f"corpus classes differ: expected {sorted(required_classes)}, got {sorted(classes)}"
        )
    if not any(task["id"] == "failed-workflow" for task in tasks):
        raise ValidationError("corpus must include failed-workflow")
    if not any(task["id"] == "cancelled-workflow" for task in tasks):
        raise ValidationError("corpus must include cancelled-workflow")

    model = corpus.get("policy", {}).get("model", {})
    if not all(model.get(field) is not None for field in ("provider", "id", "version", "temperature")):
        raise ValidationError("fixed model provider/id/version/temperature are required")

    manifest_path = ROOT / corpus["repository"]["fixture_manifest"]
    manifest = load_json(manifest_path)
    fixture_root = ROOT / corpus["repository"]["fixture_root"]
    expected_files = manifest.get("files", {})
    actual_files = {
        path.relative_to(fixture_root).as_posix()
        for path in fixture_root.rglob("*") if path.is_file()
    }
    if actual_files != set(expected_files):
        raise ValidationError("fixture manifest file set does not match fixture tree")
    for relative, expected_hash in expected_files.items():
        actual_hash = sha256(fixture_root / relative)
        if actual_hash != expected_hash:
            raise ValidationError(f"fixture hash mismatch for {relative}")

    for task in tasks:
        for evidence in task.get("expected_evidence", []):
            quote_for_evidence(corpus, evidence)

    validator, _ = validator_classes()
    schemas = ROOT / "schemas"
    for schema_path in sorted(schemas.glob("*.json")):
        schema = load_json(schema_path)
        if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            raise ValidationError(f"unsupported schema declaration in {schema_path}")
        try:
            validator.check_schema(schema)
        except Exception as error:
            raise ValidationError(f"invalid Draft 2020-12 schema {schema_path}: {error}") from error

    for key, path_key, digest_key in (
        ("executable_harness", "path", "sha256"),
        ("executable_harness", "config_path", "config_sha256"),
        ("executable_harness", "requirements_path", "requirements_sha256"),
        ("grader", "path", "sha256"),
    ):
        declaration = corpus[key]
        path = ROOT / declaration[path_key]
        if sha256(path) != declaration[digest_key]:
            raise ValidationError(f"pinned digest is stale for {path.name}")

    corpus_hash = sha256(CORPUS_PATH)
    declaration_hash = sha256(THRESHOLDS_PATH)
    if thresholds.get("declaration_status") != "pre_results":
        raise ValidationError("thresholds must remain marked pre_results")
    local_result = load_json(ROOT / "results" / "local-deterministic.json")
    if local_result.get("corpus_sha256") != corpus_hash or local_result.get("thresholds_sha256") != declaration_hash:
        raise ValidationError("local deterministic result digests are stale")
    status = load_json(ROOT / "results" / "status.json")
    if status.get("model_evaluation") != "not_run" or status.get("promotion_eligible") is not False:
        raise ValidationError("unrun model-evaluation status must remain fail closed")
    return corpus, thresholds, corpus_hash, declaration_hash


def quote_for_evidence(corpus: dict[str, Any], evidence: dict[str, Any]) -> str:
    ref_dir = corpus["repository"]["refs"].get(evidence["ref"])
    if ref_dir is None:
        raise ValidationError(f"unknown fixture ref {evidence['ref']}")
    relative = Path(evidence["path"])
    if relative.is_absolute() or ".." in relative.parts:
        raise ValidationError(f"unsafe evidence path {evidence['path']}")
    path = ROOT / corpus["repository"]["fixture_root"] / ref_dir / relative
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ValidationError(f"cannot read evidence path {path}: {error}") from error
    start, end = evidence["start_line"], evidence["end_line"]
    if not isinstance(start, int) or not isinstance(end, int) or start < 1 or end < start or end > len(lines):
        raise ValidationError(f"invalid evidence range {path}:{start}-{end}")
    return "\n".join(lines[start - 1:end])


def read_records(path: Path) -> list[dict[str, Any]]:
    records = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ValidationError(f"cannot read run records {path}: {error}") from error
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValidationError(f"invalid JSON at {path}:{line_number}: {error}") from error
        if not isinstance(record, dict):
            raise ValidationError(f"record at {path}:{line_number} is not an object")
        records.append(record)
    if not records:
        raise ValidationError("run record file is empty")
    return records


def validate_record(
    record: dict[str, Any], corpus: dict[str, Any], corpus_hash: str, threshold_hash: str,
    task_by_id: dict[str, dict[str, Any]],
) -> None:
    missing = REQUIRED_RECORD_KEYS - set(record)
    extra = set(record) - RECORD_KEYS
    if missing or extra:
        raise ValidationError(f"record keys invalid: missing={sorted(missing)}, extra={sorted(extra)}")
    validate_with_schema(record, "run-record.schema.json")
    if record["schema_version"] != 2:
        raise ValidationError("record schema_version must be 2")
    if record["benchmark_kind"] not in {"model_evaluation", "deterministic_contract"}:
        raise ValidationError("invalid benchmark_kind")
    if record["corpus_id"] != corpus["corpus_id"]:
        raise ValidationError("record corpus_id does not match corpus")
    if record["corpus_sha256"] != corpus_hash:
        raise ValidationError("record was not run against the committed corpus digest")
    if record["thresholds_sha256"] != threshold_hash:
        raise ValidationError("record was not run against the committed threshold digest")
    if record["task_id"] not in task_by_id:
        raise ValidationError(f"unknown task {record['task_id']}")
    if record["mode"] not in {"direct", "code"}:
        raise ValidationError("mode must be direct or code")
    implementation = record["implementation"]
    if not isinstance(implementation, dict) or set(implementation) != {"git_commit", "wit_mcp_sha256", "worker_sha256"}:
        raise ValidationError("implementation must contain exact build identity fields")
    if len(implementation["git_commit"]) != 40 or any(character not in "0123456789abcdef" for character in implementation["git_commit"]):
        raise ValidationError("implementation git_commit must be a lowercase 40-character SHA")
    for field in ("wit_mcp_sha256", "worker_sha256"):
        value = implementation[field]
        if value is not None and (len(value) != 64 or any(character not in "0123456789abcdef" for character in value)):
            raise ValidationError(f"implementation {field} must be a lowercase SHA-256")
    if record["mode"] == "direct" and implementation["worker_sha256"] is not None:
        raise ValidationError("direct mode worker_sha256 must be null")
    if not isinstance(record["repetition"], int) or isinstance(record["repetition"], bool) or record["repetition"] < 1:
        raise ValidationError("repetition must be a positive integer")
    if record["model"] != corpus["policy"]["model"]:
        raise ValidationError("record model policy differs from corpus")
    if record["cache"] != corpus["policy"]["cache"]:
        raise ValidationError("record cache policy differs from corpus")
    if record["status"] not in {"completed", "failed", "cancelled"}:
        raise ValidationError("invalid status")

    metrics = record["metrics"]
    if not isinstance(metrics, dict) or set(metrics) != METRIC_KEYS:
        raise ValidationError("metrics must contain exactly the declared metric fields")
    for key, value in metrics.items():
        if key == "worker_startup_ms" and value is None:
            if record["mode"] == "code":
                raise ValidationError("Code Mode must report worker_startup_ms")
            continue
        if not isinstance(value, (int, float)) or isinstance(value, bool) or value < 0:
            raise ValidationError(f"metric {key} must be a non-negative number")
        if key in NONNEGATIVE_INTEGER_METRICS and not isinstance(value, int):
            raise ValidationError(f"metric {key} must be an integer")
    if record["mode"] == "direct" and metrics["worker_startup_ms"] is not None:
        raise ValidationError("direct mode worker_startup_ms must be null")
    if not isinstance(metrics["peak_process_count"], int) or metrics["peak_process_count"] < 1:
        raise ValidationError("peak_process_count must be a positive integer")
    if metrics["invalid_calls"] > metrics["outer_mcp_calls"] + metrics["inner_host_calls"]:
        raise ValidationError("invalid_calls cannot exceed all recorded calls")

    response = record["response"]
    if not isinstance(response, dict) or set(response) != {"fact_ids", "evidence"}:
        raise ValidationError("response must contain exactly fact_ids and evidence")
    facts = response["fact_ids"]
    if not isinstance(facts, list) or len(facts) != len(set(facts)) or not all(isinstance(item, str) for item in facts):
        raise ValidationError("response fact_ids must be unique strings")
    if not isinstance(response["evidence"], list):
        raise ValidationError("response evidence must be an array")
    for evidence in response["evidence"]:
        expected_keys = {"fact_id", "ref", "path", "start_line", "end_line", "quote"}
        if not isinstance(evidence, dict) or set(evidence) != expected_keys:
            raise ValidationError("each evidence item must contain the exact evidence fields")
        if evidence["quote"] != quote_for_evidence(corpus, evidence):
            raise ValidationError(
                f"evidence quote does not match fixture at {evidence['ref']}:{evidence['path']}"
            )
    raw_without_digest = {
        "responses": record["raw"]["responses"],
        "final_output_text": record["raw"]["final_output_text"],
    }
    raw_digest = hashlib.sha256(
        json.dumps(raw_without_digest, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if raw_digest != record["raw"]["sha256"]:
        raise ValidationError("raw Responses API payload digest does not match")
    grader_path = ROOT / corpus["grader"]["path"]
    if record["grading"]["grader_version"] != corpus["grader"]["version"]:
        raise ValidationError("run used a different grader version")
    if record["grading"]["grader_sha256"] != sha256(grader_path):
        raise ValidationError("run used a different grader digest")
    graded = grade_model_output(task_by_id[record["task_id"]], record["raw"]["final_output_text"])
    if graded["response"] != record["response"] or graded["claim_mapping"] != record["grading"]["claim_mapping"]:
        raise ValidationError("checked-in grader does not reproduce response or claim mapping")
    if graded["status"] != record["status"] or graded["error_code"] != record.get("error_code"):
        raise ValidationError("checked-in grader does not reproduce status/error_code")


def evidence_key(item: dict[str, Any]) -> tuple[Any, ...]:
    return (item["fact_id"], item["ref"], item["path"], item["start_line"], item["end_line"])


def score_record(record: dict[str, Any], task: dict[str, Any]) -> dict[str, float | bool]:
    expected_facts = {item["id"] for item in task["expected_facts"]}
    actual_facts = set(record["response"]["fact_ids"])
    expected_evidence = {evidence_key(item) for item in task["expected_evidence"]}
    actual_evidence = {evidence_key(item) for item in record["response"]["evidence"]}

    outcome_matches = record["status"] == task["expected_status"]
    if task.get("expected_error_code") is not None:
        outcome_matches = outcome_matches and record.get("error_code") == task["expected_error_code"]
    union_facts = expected_facts | actual_facts
    fact_score = len(expected_facts & actual_facts) / len(union_facts) if union_facts else 1.0
    correctness = fact_score if outcome_matches else 0.0
    precision = len(expected_evidence & actual_evidence) / len(actual_evidence) if actual_evidence else (1.0 if not expected_evidence else 0.0)
    recall = len(expected_evidence & actual_evidence) / len(expected_evidence) if expected_evidence else (1.0 if not actual_evidence else 0.0)
    return {
        "correctness": correctness,
        "provenance_precision": precision,
        "provenance_recall": recall,
        "outcome_matches": outcome_matches,
    }


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


def aggregate(records: list[dict[str, Any]]) -> dict[str, Any]:
    calls = sum(record["metrics"]["outer_mcp_calls"] + record["metrics"]["inner_host_calls"] for record in records)
    invalid = sum(record["metrics"]["invalid_calls"] for record in records)
    startup = [record["metrics"]["worker_startup_ms"] for record in records if record["metrics"]["worker_startup_ms"] is not None]
    return {
        "runs": len(records),
        "tool_description_bytes_p50": percentile([record["metrics"]["tool_description_bytes"] for record in records], 0.50),
        "tool_description_tokens_p50": percentile([record["metrics"]["tool_description_tokens"] for record in records], 0.50),
        "model_visible_input_bytes_total": sum(record["metrics"]["model_visible_input_bytes"] for record in records),
        "model_visible_result_bytes_total": sum(record["metrics"]["model_visible_result_bytes"] for record in records),
        "model_cached_input_tokens_total": sum(record["metrics"]["model_cached_input_tokens"] for record in records),
        "model_visible_bytes_total": sum(record["metrics"]["model_visible_input_bytes"] + record["metrics"]["model_visible_result_bytes"] for record in records),
        "outer_mcp_calls_total": sum(record["metrics"]["outer_mcp_calls"] for record in records),
        "inner_host_calls_total": sum(record["metrics"]["inner_host_calls"] for record in records),
        "invalid_calls_total": invalid,
        "invalid_call_rate": invalid / calls if calls else 0.0,
        "wall_time_ms_p50": percentile([record["metrics"]["wall_time_ms"] for record in records], 0.50),
        "wall_time_ms_p95": percentile([record["metrics"]["wall_time_ms"] for record in records], 0.95),
        "worker_startup_ms_p50": percentile(startup, 0.50),
        "worker_startup_ms_p95": percentile(startup, 0.95),
        "peak_rss_bytes": max(record["metrics"]["peak_rss_bytes"] for record in records),
        "peak_cpu_percent": max(record["metrics"]["peak_cpu_percent"] for record in records),
        "peak_process_count": max(record["metrics"]["peak_process_count"] for record in records),
        "correctness": sum(record["score"]["correctness"] for record in records) / len(records),
        "provenance_precision": sum(record["score"]["provenance_precision"] for record in records) / len(records),
        "provenance_recall": sum(record["score"]["provenance_recall"] for record in records) / len(records),
        "expected_outcome_rate": sum(bool(record["score"]["outcome_matches"]) for record in records) / len(records),
    }


def reduction(direct: float, code: float) -> float:
    if direct == 0:
        return 0.0 if code == 0 else -1.0
    return (direct - code) / direct


def build_report(
    records: list[dict[str, Any]], corpus: dict[str, Any], thresholds: dict[str, Any], corpus_hash: str, threshold_hash: str,
) -> dict[str, Any]:
    task_by_id = {task["id"]: task for task in corpus["tasks"]}
    kinds = {record["benchmark_kind"] for record in records}
    if len(kinds) != 1:
        raise ValidationError("one report cannot mix benchmark_kind values")
    identities = set()
    pair_ids: dict[tuple[str, int], dict[str, str]] = defaultdict(dict)
    validated_records = []
    for record in records:
        validate_record(record, corpus, corpus_hash, threshold_hash, task_by_id)
        identity = (record["task_id"], record["mode"], record["repetition"])
        if identity in identities:
            raise ValidationError(f"duplicate task/mode/repetition record: {identity}")
        identities.add(identity)
        pair_ids[(record["task_id"], record["repetition"])][record["mode"]] = record["pair_id"]
        validated_records.append({**record, "score": score_record(record, task_by_id[record["task_id"]])})
    records = validated_records
    expected_repetitions = set(range(1, thresholds["promotion"]["minimum_repetitions_per_task_mode"] + 1))
    for task in corpus["tasks"]:
        for mode in ("direct", "code"):
            actual = {record["repetition"] for record in records if record["task_id"] == task["id"] and record["mode"] == mode}
            if actual != expected_repetitions:
                raise ValidationError(
                    f"{task['id']}:{mode} repetitions must be exactly {sorted(expected_repetitions)}, got {sorted(actual)}"
                )
        for repetition in expected_repetitions:
            paired = pair_ids[(task["id"], repetition)]
            if set(paired) != {"direct", "code"} or paired["direct"] != paired["code"]:
                raise ValidationError(f"unpaired direct/code records for {task['id']} repetition {repetition}")
    commits = {record["implementation"]["git_commit"] for record in records}
    server_hashes = {record["implementation"]["wit_mcp_sha256"] for record in records}
    if len(commits) != 1 or len(server_hashes) != 1:
        raise ValidationError("all comparative records must use one git commit and wit-mcp artifact")

    grouped: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    by_mode: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for record in records:
        grouped[(record["task_id"], record["mode"])].append(record)
        by_mode[record["mode"]].append(record)

    task_aggregates = {
        task_id: {mode: aggregate(grouped[(task_id, mode)]) for mode in ("direct", "code") if grouped[(task_id, mode)]}
        for task_id in task_by_id
    }
    mode_aggregates = {mode: aggregate(items) for mode, items in sorted(by_mode.items())}

    promotion = thresholds["promotion"]
    minimum = promotion["minimum_repetitions_per_task_mode"]
    coverage_missing = []
    composition_classes = set(promotion["composition_heavy"]["task_classes"])
    composition = {
        mode: [record for record in by_mode.get(mode, []) if task_by_id[record["task_id"]]["class"] in composition_classes]
        for mode in ("direct", "code")
    }
    composition_aggregates = {
        mode: aggregate(items) for mode, items in composition.items() if items
    }

    gates: list[dict[str, Any]] = []
    benchmark_kind = next(iter(kinds))
    gates.append({"name": "model_evaluation", "passed": benchmark_kind == "model_evaluation", "actual": benchmark_kind})
    gates.append({"name": "complete_repetitions", "passed": not coverage_missing, "missing": coverage_missing})

    direct = mode_aggregates.get("direct")
    code = mode_aggregates.get("code")
    if direct and code:
        gates.extend([
            {"name": "correctness_floor", "passed": code["correctness"] >= promotion["minimum_correctness"], "actual": code["correctness"]},
            {"name": "correctness_vs_direct", "passed": code["correctness"] >= direct["correctness"], "direct": direct["correctness"], "code": code["correctness"]},
            {"name": "provenance_precision_floor", "passed": code["provenance_precision"] >= promotion["minimum_provenance_precision"], "actual": code["provenance_precision"]},
            {"name": "provenance_precision_vs_direct", "passed": code["provenance_precision"] >= direct["provenance_precision"], "direct": direct["provenance_precision"], "code": code["provenance_precision"]},
            {"name": "provenance_recall_floor", "passed": code["provenance_recall"] >= promotion["minimum_provenance_recall"], "actual": code["provenance_recall"]},
            {"name": "invalid_call_rate", "passed": code["invalid_call_rate"] <= promotion["maximum_invalid_call_rate"], "actual": code["invalid_call_rate"]},
            {"name": "code_wall_time_p95", "passed": code["wall_time_ms_p95"] <= promotion["maximum_code_mode_wall_time_p95_ms"], "actual": code["wall_time_ms_p95"]},
            {"name": "worker_startup_p95", "passed": code["worker_startup_ms_p95"] is not None and code["worker_startup_ms_p95"] <= promotion["maximum_code_mode_worker_startup_p95_ms"], "actual": code["worker_startup_ms_p95"]},
        ])
    else:
        gates.append({"name": "both_modes_present", "passed": False})

    direct_composition = composition_aggregates.get("direct")
    code_composition = composition_aggregates.get("code")
    reductions = None
    if direct_composition and code_composition:
        reductions = {
            "outer_calls": reduction(direct_composition["outer_mcp_calls_total"], code_composition["outer_mcp_calls_total"]),
            "model_visible_bytes": reduction(direct_composition["model_visible_bytes_total"], code_composition["model_visible_bytes_total"]),
        }
        target = promotion["composition_heavy"]
        calls_pass = reductions["outer_calls"] >= target["minimum_outer_call_reduction"]
        bytes_pass = reductions["model_visible_bytes"] >= target["minimum_model_visible_byte_reduction"]
        passed = calls_pass or bytes_pass if target["reduction_rule"] == "either" else calls_pass and bytes_pass
        gates.append({"name": "composition_reduction", "passed": passed, "actual": reductions, "rule": target["reduction_rule"]})
        for task in corpus["tasks"]:
            if task["class"] not in composition_classes:
                continue
            task_direct = task_aggregates[task["id"]]["direct"]
            task_code = task_aggregates[task["id"]]["code"]
            actual = {
                "outer_calls": reduction(task_direct["outer_mcp_calls_total"], task_code["outer_mcp_calls_total"]),
                "model_visible_bytes": reduction(task_direct["model_visible_bytes_total"], task_code["model_visible_bytes_total"]),
            }
            task_passed = (
                actual["outer_calls"] >= target["minimum_outer_call_reduction"]
                or actual["model_visible_bytes"] >= target["minimum_model_visible_byte_reduction"]
            )
            gates.append({"name": f"composition_task:{task['id']}", "passed": task_passed, "actual": actual})
        for task_class in sorted(composition_classes):
            class_records = {
                mode: [record for record in composition[mode] if task_by_id[record["task_id"]]["class"] == task_class]
                for mode in ("direct", "code")
            }
            class_direct = aggregate(class_records["direct"])
            class_code = aggregate(class_records["code"])
            actual = {
                "outer_calls": reduction(class_direct["outer_mcp_calls_total"], class_code["outer_mcp_calls_total"]),
                "model_visible_bytes": reduction(class_direct["model_visible_bytes_total"], class_code["model_visible_bytes_total"]),
            }
            class_passed = (
                actual["outer_calls"] >= target["minimum_outer_call_reduction"]
                or actual["model_visible_bytes"] >= target["minimum_model_visible_byte_reduction"]
            )
            gates.append({"name": f"composition_class:{task_class}", "passed": class_passed, "actual": actual})
    else:
        gates.append({"name": "composition_reduction", "passed": False, "actual": None})

    simple_reported = all(grouped[("simple-open-read", mode)] for mode in ("direct", "code"))
    gates.append({"name": "simple_task_reported", "passed": simple_reported})
    failure_records = [record for record in records if task_by_id[record["task_id"]]["class"] == "failure_and_cancellation"]
    failure_pass = bool(failure_records) and all(record["score"]["outcome_matches"] for record in failure_records)
    gates.append({"name": "failure_and_cancellation", "passed": failure_pass})

    eligible = benchmark_kind == "model_evaluation" and not coverage_missing
    passed = eligible and all(gate["passed"] for gate in gates)
    report = {
        "schema_version": 2,
        "corpus_id": corpus["corpus_id"],
        "corpus_sha256": corpus_hash,
        "benchmark_kind": benchmark_kind,
        "thresholds_sha256": threshold_hash,
        "implementations": [
            {"git_commit": commit, "wit_mcp_sha256": server_hash, "worker_sha256": worker_hash or None}
            for commit, server_hash, worker_hash in sorted({
                (record["implementation"]["git_commit"], record["implementation"]["wit_mcp_sha256"], record["implementation"]["worker_sha256"] or "")
                for record in records
            })
        ],
        "run_count": len(records),
        "policy": {"model": corpus["policy"]["model"], "cache": corpus["policy"]["cache"], "measurement": corpus["policy"]["measurement"]},
        "aggregates": {"modes": mode_aggregates, "tasks": task_aggregates, "composition_heavy": composition_aggregates},
        "comparison": {"composition_reductions": reductions},
        "promotion": {
            "eligible": eligible,
            "passed": passed,
            "code_mode_status": "promotable" if passed else thresholds["failure_policy"]["code_mode_status_when_any_gate_fails"],
            "recommendation": "code" if passed else thresholds["failure_policy"]["recommendation_when_any_gate_fails"],
            "gates": gates,
        },
    }
    validate_with_schema(report, "report.schema.json")
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("validate", help="validate corpus, fixtures, schemas, and thresholds")
    report_parser = subparsers.add_parser("report", help="validate JSONL run records and write a comparative report")
    report_parser.add_argument("records", type=Path)
    report_parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)

    try:
        corpus, thresholds, corpus_hash, threshold_hash = validate_package()
        if args.command == "validate":
            result = {"valid": True, "corpus_id": corpus["corpus_id"], "corpus_sha256": corpus_hash, "thresholds_sha256": threshold_hash, "task_count": len(corpus["tasks"])}
        else:
            result = build_report(read_records(args.records), corpus, thresholds, corpus_hash, threshold_hash)
    except ValidationError as error:
        print(f"benchmark validation failed: {error}", file=sys.stderr)
        return 2

    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if getattr(args, "output", None):
        args.output.write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
