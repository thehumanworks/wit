#!/bin/sh
set -eu

benchmark_python=${BENCHMARK_PYTHON:-python3}

"$benchmark_python" benchmarks/codemode/runner.py validate
"$benchmark_python" -m unittest discover -s benchmarks/codemode/tests -p 'test_*.py'
