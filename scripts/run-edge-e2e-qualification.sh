#!/usr/bin/env bash
# Invoke a deployed edge function through the gateway, against the real stack.
#
# This needs a container engine because the function executes inside the pinned
# Supabase Edge Runtime, which is why it is opt-in rather than part of the
# ordinary edit loop.
set -euo pipefail

if [[ "${MAKO_RUN_EDGE_RUNTIME_TESTS:-}" != "1" ]]; then
  echo "Set MAKO_RUN_EDGE_RUNTIME_TESTS=1 and configure Docker or Podman first." >&2
  exit 2
fi

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
rust_tmp="${MAKO_STORAGE_TMPDIR:-$repository_root/.playwright-tmp/rust}"
mkdir -p "$rust_tmp"
chmod 0700 "$rust_tmp"
export TMPDIR="$rust_tmp"
export MAKO_STORAGE_TMPDIR="$rust_tmp"

engine="${MAKO_EDGE_TEST_ENGINE:-docker}"
image=$(node -e 'const p=require("./infra/edge-runtime/runtime-pin.json");process.stdout.write(`${p.imageRepository}@${p.imageDigest}`)')
output="${1:-$repository_root/docs/evidence/edge-e2e-qualification.json}"
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Pull by digest up front so a slow first pull is not mistaken for a hang.
read -r -a engine_prefix <<< "$(node -e 'const a=JSON.parse(process.env.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON??"[]");process.stdout.write(a.join(" "))')"
# `set -u` on bash 3.2, which macOS still ships, treats an empty array
# expansion as an unset variable, so the empty case needs the guarded form.
"$engine" "${engine_prefix[@]+"${engine_prefix[@]}"}" pull "$image"

cargo build --workspace --bins
cargo test -p mako-smoke --test edge_function -- --nocapture

completed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
mkdir -p "$(dirname "$output")"
cat > "$output" <<JSON
{
  "schemaVersion": 1,
  "qualification": "hosted-edge-function-invocation",
  "status": "passed",
  "startedAt": "$started_at",
  "completedAt": "$completed_at",
  "host": {
    "kernel": "$(uname -s)",
    "release": "$(uname -r)",
    "architecture": "$(uname -m)",
    "containerEngine": "$engine"
  },
  "runtimeImage": "$image",
  "scope": {
    "profile": "debug",
    "services": ["mako-data-plane", "mako-control-plane", "mako-edge-gateway"],
    "transport": "real HTTP over loopback, no mocked or intercepted requests",
    "covered": [
      "function deployed through the administrative path and registered with the supervisor",
      "invocation through the edge gateway returns the function's own response",
      "an undeployed function is not served",
      "a project reference without an environment does not resolve"
    ],
    "notCovered": [
      "JWT-verified invocation, function secrets, and outbound egress policy",
      "regional failover and quota enforcement",
      "promotion, rollback, and retirement of a deployed version"
    ]
  },
  "test": "crates/mako-smoke/tests/edge_function.rs"
}
JSON

echo "wrote $output"
