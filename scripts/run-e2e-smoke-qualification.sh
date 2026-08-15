#!/usr/bin/env bash
# Build the service binaries and drive the application happy path against them,
# then record what was exercised and on which host.
#
# The assertions live in the Rust test; this wrapper adds none of its own, so
# there is one definition of what passing means.
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"
mkdir -p "$rust_tmp"
chmod 0700 "$rust_tmp"
export TMPDIR="$rust_tmp"
export MAKO_STORAGE_TMPDIR="$rust_tmp"

output="${1:-$repository_root/docs/evidence/e2e-smoke-qualification.json}"
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)

cargo build --workspace --bins
cargo test -p mako-smoke --test happy_path -- --nocapture

completed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
mkdir -p "$(dirname "$output")"
cat > "$output" <<JSON
{
  "schemaVersion": 1,
  "qualification": "application-happy-path-smoke",
  "status": "passed",
  "startedAt": "$started_at",
  "completedAt": "$completed_at",
  "host": {
    "kernel": "$(uname -s)",
    "release": "$(uname -r)",
    "architecture": "$(uname -m)"
  },
  "scope": {
    "profile": "debug",
    "services": ["mako-data-plane", "mako-control-plane"],
    "transport": "real HTTP over loopback, no mocked or intercepted requests",
    "covered": [
      "local tenant bootstrap",
      "application-user sign-up",
      "application-user sign-in",
      "document replication push",
      "document replication pull returning the pushed document",
      "unauthenticated document access is refused"
    ],
    "notCovered": [
      "hosted developer registration, email verification, and operator wait-list review",
      "management API project, environment, and collection administration",
      "edge functions, object store, telemetry, and console user interface"
    ]
  },
  "test": "crates/mako-smoke/tests/happy_path.rs"
}
JSON

echo "wrote $output"
