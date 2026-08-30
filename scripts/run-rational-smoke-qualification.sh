#!/usr/bin/env bash
# Build the service binaries and drive Rational -- the sample application --
# against them, then record what was exercised and on which host.
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

output="${1:-$repository_root/docs/evidence/rational-smoke-qualification.json}"
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)

cargo build --workspace --bins
cargo test -p mako-smoke --test rational -- --nocapture

completed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
mkdir -p "$(dirname "$output")"
cat > "$output" <<JSON
{
  "schemaVersion": 1,
  "qualification": "rational-sample-application-smoke",
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
      "the sample application's own model published from examples/rational/mako",
      "sign-in by password, by an OpenID Connect provider, and by magic link",
      "a household shared by claim, written by trusted code through the service route",
      "an outsider refused the household's documents, reading none of them",
      "a CSV-shaped import of many transactions in one push",
      "a categorization rule recorded on the transaction it filed",
      "a receipt readable by another member and refused to an outsider",
      "an alert written by trusted code, replicated to the household, and delivered once, signed",
      "writes queued while a device was away, accepted on reconnect"
    ],
    "notCovered": [
      "the edge functions, which need the pinned runtime (see the edge-runtime qualification)",
      "the browser: the screens are covered by the sample's own Playwright suites"
    ]
  },
  "test": "crates/mako-smoke/tests/rational.rs"
}
JSON

echo "wrote $output"
