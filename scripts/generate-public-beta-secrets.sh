#!/usr/bin/env bash
set -euo pipefail
umask 077

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
secret_root="${1:-${repository_root}/.local/public-beta-secrets}"
/usr/bin/install -d -m 0700 "$secret_root"

generate_secret() {
  local name=$1
  local destination="${secret_root}/${name}"
  if [[ -f "$destination" ]]; then
    [[ $(/usr/bin/stat -c %a "$destination") == 600 ]]
    [[ $(/usr/bin/wc -c <"$destination") -eq 65 ]]
    return
  fi

  local temporary
  temporary=$(/usr/bin/mktemp "${secret_root}/.${name}.XXXXXX")
  trap '/usr/bin/rm -f "$temporary"' RETURN
  /usr/bin/openssl rand -hex -out "$temporary" 32
  /usr/bin/chmod 0600 "$temporary"
  /usr/bin/mv --no-clobber "$temporary" "$destination"
  trap - RETURN
}

generate_secret internal-auth
generate_secret runtime-state-key
generate_secret object-store-access-key
generate_secret object-store-secret-key
generate_secret backup-signing-key
generate_secret grafana-admin-password
generate_secret developer-mail-encryption

echo "public-beta secret sources are present with protected modes"
