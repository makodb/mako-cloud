#!/usr/bin/env bash
set -euo pipefail

if [[ "${MAKO_RUN_EDGE_RUNTIME_TESTS:-}" != "1" ]]; then
  echo "Set MAKO_RUN_EDGE_RUNTIME_TESTS=1 and configure Docker or Podman before release qualification." >&2
  exit 2
fi

cargo test \
  -p mako-edge-runtime-protocol \
  -p mako-edge-runtime \
  -p mako-edge-gateway
npm audit --audit-level=high
npm run test:unit --workspace @mako-cloud/cli
npm run test:integration --workspace @mako-cloud/cli
