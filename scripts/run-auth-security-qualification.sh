#!/usr/bin/env bash
set -euo pipefail

cargo test \
  -p mako-identity \
  -p mako-gateway \
  -p mako-control-plane \
  -p mako-control-plane-service
npm run test:unit --workspace @mako-cloud/rxdb
