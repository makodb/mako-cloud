#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
benchmark_output="${MAKO_BENCH_OUTPUT:-$repository_root/.playwright-tmp/performance-benchmark.json}"
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"

mkdir -p "$(dirname "$benchmark_output")" "$rust_tmp"

cd "$repository_root"
env \
  TMPDIR="$rust_tmp" \
  CARGO_INCREMENTAL=0 \
  cargo run --release --locked -p mako-benchmarks -- --output "$benchmark_output"
node scripts/validate-performance-baseline.js "$benchmark_output"

echo "Performance report: $benchmark_output"
