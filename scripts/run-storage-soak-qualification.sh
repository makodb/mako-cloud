#!/usr/bin/env bash
set -euo pipefail

iterations="${MAKO_STORAGE_SOAK_ITERATIONS:-25}"
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"
if [[ ! "$iterations" =~ ^[1-9][0-9]*$ ]]; then
  echo "MAKO_STORAGE_SOAK_ITERATIONS must be a positive integer" >&2
  exit 2
fi

mkdir -p "$rust_tmp"
cd "$repository_root"
export TMPDIR="$rust_tmp"
export CARGO_INCREMENTAL=0

for ((iteration = 1; iteration <= iterations; iteration++)); do
  echo "storage soak iteration ${iteration}/${iterations}"
  cargo test --quiet \
    -p mako-storage \
    -p mako-documents
done
