#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repository_root"

cargo fmt --all -- --check
cargo test --locked -p mako-config -p mako-internal-rpc -p mako-control-plane -p mako-control-plane-service
npm run generate:api:check
npm run typecheck
npm run test:unit --workspace @mako-cloud/management-sdk
npm run test:unit --workspace @mako-cloud/console
npm run test:e2e --workspace @mako-cloud/console -- --grep operator
npm run validate:operator-auth-assets
npm run validate:public-beta-caddy
npm run test:operator-session-cleanup
