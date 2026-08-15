#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
  echo "usage: verify-public-beta-secret-redaction LOG SECRET_DIRECTORY" >&2
  exit 64
fi

log=$1
secret_root=$2
for name in internal-auth object-store-access-key object-store-secret-key backup-signing-key grafana-admin-password developer-mail-encryption developer-smtp-password; do
  source_file="${secret_root}/${name}"
  if [[ ! -s "$source_file" ]]; then
    echo "required secret source is unavailable" >&2
    exit 1
  fi
  if /usr/bin/grep --fixed-strings --quiet --file "$source_file" "$log"; then
    echo "secret material was disclosed in the deployment log" >&2
    exit 1
  fi
done

echo "verified public-beta deployment log redaction"
