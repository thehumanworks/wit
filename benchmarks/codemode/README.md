# Direct MCP versus Code Mode benchmark

This package defines the pre-results benchmark contract for
[`thehumanworks/wit#20`](https://github.com/thehumanworks/wit/issues/20). It does
not contain a completed model evaluation. Until a complete report passes every
predeclared gate, Code Mode remains experimental and direct MCP is recommended.

## Fixed experiment

`corpus.json` fixes the model snapshot and temperature, prompt policy,
result budget, cold-cache reset policy, repetitions, local repository refs, and
seven tasks across the six required classes. Both modes receive exactly the same
task prompt and fixture. The only permitted mode-specific model input is the MCP
server's own tool descriptions and schemas.

The fixture is network-independent. `fixture-manifest.json` pins every byte by
SHA-256, while `base` and `target` provide the two immutable logical refs. Each
task declares grader-only facts and exact inclusive line ranges used to score
correctness, provenance precision, and provenance recall. The failure class has separate
structured-failure and cancellation tasks so neither outcome can be inferred
from the other.

`thresholds.json` is deliberately checked in while `results/status.json` says
the model evaluation is unrun. Its digest is required in every run record, so a
report cannot silently use thresholds chosen after collection. The promotion
gates require perfect correctness and provenance, no regression from direct
mode, at least 30% fewer outer calls or 25% fewer model-visible bytes on the four
composition-heavy classes, Code Mode wall-time p95 at or below 12 seconds,
worker-startup p95 at or below 750 ms, and invalid calls at or below 2%.

## Pinned executable adapter

`harness.py` is the sole model adapter. `corpus.json` pins its bytes, its
`harness-config.json`, the deterministic grader, and the complete Python lock
file. Install the pinned environment outside the repository:

```sh
python3 -m venv /tmp/wit-codemode-benchmark
/tmp/wit-codemode-benchmark/bin/pip install -r benchmarks/codemode/requirements.lock
```

The adapter speaks MCP JSON-RPC over stdio: `initialize`,
`notifications/initialized`, `tools/list`, `tools/call`, and, for the cancelled
task, `notifications/cancelled`. MCP tools are exposed to the pinned OpenAI
Responses API model as function tools. Each response's complete `output` array
is retained in the next stateless request and each tool result is appended as a
`function_call_output` with the original `call_id`, following OpenAI's
[function-calling flow](https://developers.openai.com/api/docs/guides/function-calling).
API storage and parallel calls are disabled.

Each run gets a new temporary Git remote and cache. `base` and `target` are
created from the checked-in fixture, GitHub URLs are rewritten to that local
remote, and dead proxy settings prevent repository traffic. The parent adapter
is allowed to reach only the configured OpenAI API endpoint. Locale, timezone,
Python/package versions, OS/architecture, Git version, commands, binary hashes,
fixture commit SHAs, and cache policy are recorded.

The MCP server must write the required instrumentation sidecar named by
`WIT_BENCHMARK_METRICS_FILE`. Missing credentials, artifacts, exact package
versions, instrumentation, tools, or API completion cause exit 3 before any run
record is written. Peak RSS, CPU, and process count are sampled across the MCP
process tree every 10 ms. Tool-description tokens use pinned `tiktoken==0.12.0`
with `o200k_base`. A unique prompt-cache key isolates every task/mode/repetition;
the API-reported cached-input token count remains visible in every report.

## Run procedure

The pinned `harness.py` adapter must perform these steps without changing the corpus:

1. Build the exact `wit-mcp` artifact at a committed revision. The pinned
   commands are `target/release/wit-mcp --mode direct` and `--mode code`.
2. Set `OPENAI_API_KEY`. For each task and repetition 1 through 10, run both
   modes with the same pair ID. For example:

   ```sh
   /tmp/wit-codemode-benchmark/bin/python benchmarks/codemode/harness.py \
     --task simple-open-read --mode direct --repetition 1 \
     --output runs/simple-open-read-direct-1.jsonl
   ```

   The harness refuses to overwrite a record. Concatenate completed record
   files only after the full 140-record matrix exists.
3. Supply the fixed system prompt plus task prompt. Do not retry, add hints, or
   carry conversation/cache state between repetitions.
4. Grade the returned claims against the checked-in expected facts without
   exposing fact IDs or expected evidence to the model, then capture one JSON
   object matching `schemas/run-record.schema.json`. Measure
   description bytes on the serialized `tools/list` result; description tokens
   with the model adapter's native tokenizer; model-visible bytes at the adapter
   boundary; outer MCP calls at the client boundary; and inner host calls at the
   Code Mode parent dispatcher (or direct operation handler).
5. Measure end-to-end wall time from prompt submission to final answer. For Code
   Mode, also measure child spawn to ready/first protocol message. Sample the
   complete server/worker process tree every 10 ms and record peak RSS, CPU
   percent, and process count. Direct mode uses `null` for worker startup.
6. Write newline-delimited records and generate the report:

   ```sh
   python3 benchmarks/codemode/runner.py report runs.jsonl --output report.json
   ```

The runner rejects changed model/cache policies, changed corpus or threshold
digests, duplicate or unpaired repetitions, incomplete metric fields, and evidence
quotes that do not match the pinned fixture. It retains raw API responses and
the final model output, then reruns the versioned, SHA-256-pinned grader and
claim-to-fact mapping. It reports nearest-rank wall/startup p50 and p95,
description bytes/tokens, model-visible input and result bytes, outer and inner
calls, invalid-call rate, and peak resources by mode and task.

Both schemas are enforced by pinned `jsonschema==4.25.1` using
`Draft202012Validator`; the validator checks each schema itself, every input
record, and the generated report. Repetitions must be exactly 1 through 10 for
both modes with identical pair IDs. The reduction threshold must pass for the
aggregate, every composition-heavy task, and every composition-heavy class.

## Deterministic local gate

The software-level gate uses only Python's standard library:

```sh
BENCHMARK_PYTHON=/tmp/wit-codemode-benchmark/bin/python \
  scripts/check_codemode_benchmark.sh
```

It verifies fixture hashes and evidence ranges, parses the schemas, exercises
report aggregation and every promotion decision, and proves that records marked
`deterministic_contract` cannot promote Code Mode. These are local deterministic
runner results, not model-quality, MCP integration, latency, or resource results.
The checked-in outcome is `results/local-deterministic.json`; the separate
`results/status.json` keeps the external model evaluation explicitly unrun.

## Interpreting results

Simple open/read is always included even if direct MCP wins. Correctness and
provenance are independent of efficiency: lower bytes or fewer calls cannot
compensate for a missed fact, imprecise citation, wrong failure code, or broken
cancellation. Missing/incomplete/non-model records or any failed gate produce
`code_mode_status: experimental` and `recommendation: direct`; unfavorable
results must remain visible rather than being relabeled as a pass.
