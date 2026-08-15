#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"
mkdir -p "$rust_tmp"
chmod 0700 "$rust_tmp"
export TMPDIR="$rust_tmp"

cargo test \
  -p mako-policy \
  -p mako-sync \
  -p mako-gateway
npm run test:unit --workspace @mako-cloud/edge-sdk
