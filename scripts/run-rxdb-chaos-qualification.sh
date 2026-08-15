#!/usr/bin/env bash
set -euo pipefail

cargo test \
  -p mako-sync \
  -p mako-documents
npm run test:unit --workspace @mako-cloud/rxdb
