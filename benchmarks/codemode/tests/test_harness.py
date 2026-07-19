import importlib.util
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("codemode_benchmark_harness", ROOT / "harness.py")
harness = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(harness)


class HarnessTests(unittest.TestCase):
    def test_mcp_tools_are_wired_as_responses_function_tools(self):
        schema = {"type": "object", "properties": {"repo": {"type": "string"}}, "required": ["repo"]}
        tools = harness.response_tools([{"name": "wit_open", "description": "open", "inputSchema": schema}])
        self.assertEqual(tools, [{
            "type": "function", "name": "wit_open", "description": "open",
            "parameters": schema, "strict": False,
        }])

    def test_responses_payload_is_stateless_and_pinned(self):
        inputs = [{"role": "user", "content": "task"}]
        tools = [{"type": "function", "name": "tool", "parameters": {}}]
        payload = harness.response_payload(inputs, "instructions", tools, "cache-key")
        self.assertEqual(payload["model"], harness.CORPUS["policy"]["model"]["id"])
        self.assertEqual(payload["input"], inputs)
        self.assertEqual(payload["tools"], tools)
        self.assertFalse(payload["store"])
        self.assertFalse(payload["parallel_tool_calls"])
        self.assertEqual(payload["truncation"], "disabled")
        self.assertEqual(payload["prompt_cache_key"], "cache-key")

    def test_function_call_output_preserves_call_id_and_exact_bytes(self):
        item, byte_count = harness.function_call_output("call-1", {"answer": 1})
        self.assertEqual(item, {"type": "function_call_output", "call_id": "call-1", "output": '{"answer":1}'})
        self.assertEqual(byte_count, len(item["output"].encode()))

    def test_missing_credential_fails_without_writing_record(self):
        task = harness.CORPUS["tasks"][0]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.jsonl"
            with mock.patch.dict(os.environ, {}, clear=True):
                with self.assertRaisesRegex(harness.HarnessError, "OPENAI_API_KEY"):
                    harness.execute(task, "direct", 1, output)
            self.assertFalse(output.exists())

    def test_fixture_seed_creates_exact_base_and_target_refs(self):
        with tempfile.TemporaryDirectory() as directory:
            cache, git_config, refs = harness.seed_repository(Path(directory))
            self.assertTrue(cache.is_dir())
            self.assertTrue(git_config.is_file())
            self.assertEqual(set(refs), {"base", "target"})
            self.assertNotEqual(refs["base"], refs["target"])

    def test_mcp_environment_excludes_model_credentials_and_blocks_repository_network(self):
        with mock.patch.dict(os.environ, {"OPENAI_API_KEY": "secret", "PATH": "/bin"}, clear=True):
            environment = harness.benchmark_subprocess_environment(
                Path("/cache"), Path("/gitconfig"), Path("/metrics"),
            )
        self.assertNotIn("OPENAI_API_KEY", environment)
        self.assertEqual(environment["PATH"], "/bin")
        self.assertEqual(environment["HTTPS_PROXY"], "http://127.0.0.1:9")


if __name__ == "__main__":
    unittest.main()
