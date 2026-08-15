#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"
mkdir -p "$rust_tmp"
chmod 0700 "$rust_tmp"
export TMPDIR="$rust_tmp"

cargo test \
  -p mako-storage \
  -p mako-gateway \
  -p mako-control-plane \
  -p mako-control-plane-service \
  -p mako-audit \
  -p mako-object-store \
  -p mako-edge-runtime
