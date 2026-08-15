#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"

mkdir -p "$rust_tmp"
cd "$repository_root"
export TMPDIR="$rust_tmp"
export CARGO_INCREMENTAL=0

npm run validate:production-storage
npm run validate:observability
cargo test --locked -p mako-storage -- --test-threads=1
MAKO_STORAGE_SOAK_ITERATIONS="${MAKO_STORAGE_SOAK_ITERATIONS:-25}" \
  npm run test:storage-soak
npm run benchmark
npm run validate:production-release
