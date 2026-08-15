#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)

mkdir -p \
  "$project_root/.local/data/mako-data-plane/rocksdb" \
  "$project_root/.local/data/mako-control-plane/rocksdb" \
  "$project_root/.local/backups/mako-data-plane" \
  "$project_root/.local/backups/mako-control-plane" \
  "$project_root/.local/backups/mako-control-plane/staging" \
  "$project_root/.local/backups/mako-control-plane/published" \
  "$project_root/.local/migrations/mako-control-plane" \
  "$project_root/.local/restores/mako-control-plane" \
  "$project_root/.local/reserve/mako-control-plane" \
  "$project_root/.local/data/object-store" \
  "$project_root/.local/certs"

# Local development secrets. Configuration accepts only references, never inline
# values, so these are generated once into files that .env.example points at with
# file: references. Regenerating them would invalidate state signed by the old
# values, so an existing secret is always left alone.
secrets_dir="$project_root/.local/secrets"
mkdir -p "$secrets_dir"
chmod 0700 "$secrets_dir"

generate_secret() {
  local name="$1"
  local path="$secrets_dir/$name"
  if [ -s "$path" ]; then
    return 0
  fi
  openssl rand -hex 32 > "$path"
  chmod 0600 "$path"
  echo "generated local secret: .local/secrets/$name"
}

generate_secret internal-auth
generate_secret object-store-access-key
generate_secret object-store-secret-key

"$script_dir/generate-certs.sh"
