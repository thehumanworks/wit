import copy
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("codemode_benchmark_runner", ROOT / "runner.py")
runner = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(runner)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.corpus, self.thresholds, self.corpus_hash, self.threshold_hash = runner.validate_package()
        self.tasks = {task["id"]: task for task in self.corpus["tasks"]}

    def record(self, task_id, mode, repetition=1, kind="model_evaluation"):
        task = self.tasks[task_id]
        evidence = []
        for expected in task["expected_evidence"]:
            item = copy.deepcopy(expected)
            item["quote"] = runner.quote_for_evidence(self.corpus, item)
            evidence.append(item)
        composition = task["composition_heavy"]
        if mode == "direct":
            outer_calls = 4 if composition else 1
            visible_input, visible_result = 1000, 500
            startup = None
        else:
            outer_calls = 1 if composition else 2
            visible_input, visible_result = 700, 300
            startup = 100
        final_output = json.dumps({
            "status": task["expected_status"],
            "error_code": task.get("expected_error_code"),
            "claims": [
                {
                    "text": fact["text"],
                    "evidence": [
                        {key: value for key, value in cited.items() if key != "fact_id"}
                        for cited in evidence if cited["fact_id"] == fact["id"]
                    ],
                }
                for fact in task["expected_facts"]
            ],
        }, sort_keys=True)
        raw_without_digest = {
            "responses": [{"id": "response-test", "status": "completed", "output": []}],
            "final_output_text": final_output,
        }
        raw_digest = hashlib.sha256(
            json.dumps(raw_without_digest, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        ).hexdigest()
        graded = runner.grade_model_output(task, final_output)
        return {
            "schema_version": 2,
            "benchmark_kind": kind,
            "run_id": f"test-{task_id}-{mode}-{repetition}",
            "pair_id": f"test-{task_id}-{repetition}",
            "corpus_id": self.corpus["corpus_id"],
            "corpus_sha256": self.corpus_hash,
            "thresholds_sha256": self.threshold_hash,
            "implementation": {
                "git_commit": "1" * 40,
                "wit_mcp_sha256": "2" * 64,
                "worker_sha256": None if mode == "direct" else "3" * 64,
            },
            "task_id": task_id,
            "mode": mode,
            "repetition": repetition,
            "model": copy.deepcopy(self.corpus["policy"]["model"]),
            "cache": copy.deepcopy(self.corpus["policy"]["cache"]),
            "environment": {
                "platform": "test", "machine": "test", "python": "3.test",
                "git": "git version test", "tiktoken": "0.12.0",
                "jsonschema": "4.25.1", "locale": "C", "timezone": "UTC",
                "commands": {"direct": ["wit-mcp", "--mode", "direct"], "code": ["wit-mcp", "--mode", "code"]},
                "fixture_refs": {"base": "4" * 40, "target": "5" * 40},
                "mcp_startup_ms": 10,
            },
            "status": task["expected_status"],
            "error_code": task.get("expected_error_code"),
            "metrics": {
                "tool_description_bytes": 6500 if mode == "direct" else 4000,
                "tool_description_tokens": 1700 if mode == "direct" else 1050,
                "model_visible_input_bytes": visible_input,
                "model_visible_result_bytes": visible_result,
                "model_cached_input_tokens": 0,
                "outer_mcp_calls": outer_calls,
                "inner_host_calls": outer_calls if mode == "direct" else (4 if composition else 1),
                "invalid_calls": 0,
                "wall_time_ms": 100 if mode == "direct" else 200,
                "worker_startup_ms": startup,
                "peak_rss_bytes": 40_000_000 if mode == "direct" else 60_000_000,
                "peak_cpu_percent": 25.0,
                "peak_process_count": 1 if mode == "direct" else 2,
            },
            "raw": {**raw_without_digest, "sha256": raw_digest},
            "grading": {
                "grader_version": self.corpus["grader"]["version"],
                "grader_sha256": runner.sha256(ROOT / self.corpus["grader"]["path"]),
                "claim_mapping": graded["claim_mapping"],
            },
            "response": graded["response"],
        }

    def test_package_has_fixed_six_classes_and_valid_fixture_hashes(self):
        self.assertEqual(len({task["class"] for task in self.corpus["tasks"]}), 6)
        self.assertEqual(len(self.corpus["tasks"]), 7)
        self.assertEqual(self.thresholds["declaration_status"], "pre_results")

    def test_complete_model_evaluation_can_pass_predeclared_gates(self):
        records = [
            self.record(task["id"], mode, repetition)
            for task in self.corpus["tasks"]
            for mode in ("direct", "code")
            for repetition in range(1, 11)
        ]
        report = runner.build_report(records, self.corpus, self.thresholds, self.corpus_hash, self.threshold_hash)
        self.assertTrue(report["promotion"]["eligible"])
        self.assertTrue(report["promotion"]["passed"])
        self.assertEqual(report["promotion"]["recommendation"], "code")
        self.assertEqual(report["aggregates"]["modes"]["code"]["worker_startup_ms_p95"], 100)

    def test_deterministic_contract_records_cannot_promote_code_mode(self):
        records = [
            self.record(task["id"], mode, repetition, kind="deterministic_contract")
            for task in self.corpus["tasks"] for mode in ("direct", "code")
            for repetition in range(1, 11)
        ]
        report = runner.build_report(records, self.corpus, self.thresholds, self.corpus_hash, self.threshold_hash)
        self.assertFalse(report["promotion"]["eligible"])
        self.assertFalse(report["promotion"]["passed"])
        self.assertEqual(report["promotion"]["code_mode_status"], "experimental")
        self.assertEqual(report["promotion"]["recommendation"], "direct")

    def test_quote_must_match_exact_fixture_lines(self):
        record = self.record("simple-open-read", "direct")
        record["response"]["evidence"][0]["quote"] = "fabricated"
        with self.assertRaisesRegex(runner.ValidationError, "quote does not match"):
            runner.validate_record(record, self.corpus, self.corpus_hash, self.threshold_hash, self.tasks)

    def test_jsonl_report_command_writes_machine_readable_report(self):
        records = [
            self.record(task["id"], mode, repetition, kind="deterministic_contract")
            for task in self.corpus["tasks"] for mode in ("direct", "code")
            for repetition in range(1, 11)
        ]
        with tempfile.TemporaryDirectory() as directory:
            input_path = Path(directory) / "runs.jsonl"
            output_path = Path(directory) / "report.json"
            input_path.write_text("".join(json.dumps(record) + "\n" for record in records), encoding="utf-8")
            exit_code = runner.main(["report", str(input_path), "--output", str(output_path)])
            self.assertEqual(exit_code, 0)
            report = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertEqual(report["run_count"], 140)
            self.assertIn("model_visible_bytes_total", report["aggregates"]["modes"]["direct"])

    def test_repetitions_must_be_exactly_paired_one_through_ten(self):
        records = [
            self.record(task["id"], mode, repetition)
            for task in self.corpus["tasks"] for mode in ("direct", "code")
            for repetition in range(1, 11)
        ]
        records[-1]["pair_id"] = "wrong-pair"
        with self.assertRaisesRegex(runner.ValidationError, "unpaired direct/code"):
            runner.build_report(records, self.corpus, self.thresholds, self.corpus_hash, self.threshold_hash)

    def test_composition_reduction_must_pass_each_task(self):
        records = [
            self.record(task["id"], mode, repetition)
            for task in self.corpus["tasks"] for mode in ("direct", "code")
            for repetition in range(1, 11)
        ]
        for record in records:
            if record["task_id"] == "symbol-search-read" and record["mode"] == "code":
                record["metrics"]["outer_mcp_calls"] = 4
                record["metrics"]["model_visible_input_bytes"] = 1000
                record["metrics"]["model_visible_result_bytes"] = 500
        report = runner.build_report(records, self.corpus, self.thresholds, self.corpus_hash, self.threshold_hash)
        gate = next(item for item in report["promotion"]["gates"] if item["name"] == "composition_task:symbol-search-read")
        self.assertFalse(gate["passed"])
        self.assertFalse(report["promotion"]["passed"])


if __name__ == "__main__":
    unittest.main()
