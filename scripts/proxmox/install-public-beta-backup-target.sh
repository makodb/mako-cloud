#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
public_key_path=${1:-$repository_root/.local/public-beta-backup/id_ed25519.pub}
[[ -f "$public_key_path" && ! -L "$public_key_path" ]] || {
  echo "backup public key must be a regular file: $public_key_path" >&2
  exit 2
}
public_key=$(<"$public_key_path")
[[ "$public_key" =~ ^ssh-ed25519\ [A-Za-z0-9+/=]+\ mako-cloud-public-beta-backup$ ]] || {
  echo "backup public key has an unexpected type or comment" >&2
  exit 2
}
encoded_key=$(printf '%s' "$public_key" | base64 -w 0)

ssh -o BatchMode=yes root@localhost bash -s -- "$encoded_key" <<'REMOTE_SCRIPT'
set -euo pipefail
encoded_key=$1
backup_user=mako-beta-backup
backup_home=/var/lib/mako-beta-backup
backup_root=/var/lib/vz/dump/mako-cloud-public-beta
command_prefix='restrict,command="/usr/bin/rrsync -no-del -no-overwrite /var/lib/vz/dump/mako-cloud-public-beta" '
public_key=$(printf '%s' "$encoded_key" | base64 -d)

command -v rrsync >/dev/null
available=$(df --output=avail -B1 /var/lib/vz/dump | tail -1 | tr -d ' ')
[[ "$available" =~ ^[0-9]+$ && "$available" -ge 274877906944 ]] || {
  echo "backup target has less than the required 256 GiB available" >&2
  exit 1
}
if ! getent passwd "$backup_user" >/dev/null; then
  useradd --system --home-dir "$backup_home" --create-home --shell /bin/sh "$backup_user"
fi
actual_home=$(getent passwd "$backup_user" | cut -d: -f6)
[[ "$actual_home" == "$backup_home" ]] || {
  echo "existing backup account has an unexpected home" >&2
  exit 1
}
passwd --lock "$backup_user" >/dev/null
install -d -o "$backup_user" -g "$backup_user" -m 0700 "$backup_home" "$backup_home/.ssh"
install -d -o "$backup_user" -g "$backup_user" -m 0700 "$backup_root"
install -d -o "$backup_user" -g "$backup_user" -m 0700 \
  "$backup_root/data-plane" \
  "$backup_root/control-plane" \
  "$backup_root/qualification"
authorized_keys=$(mktemp "$backup_home/.ssh/authorized_keys.XXXXXX")
trap 'rm -f -- "$authorized_keys"' EXIT
printf '%s%s\n' "$command_prefix" "$public_key" >"$authorized_keys"
chown "$backup_user:$backup_user" "$authorized_keys"
chmod 0600 "$authorized_keys"
mv -f "$authorized_keys" "$backup_home/.ssh/authorized_keys"
trap - EXIT
fingerprint=$(ssh-keygen -lf "$backup_home/.ssh/authorized_keys" | awk '{print $2}')
echo "configured restricted off-VM backup target $backup_root for key $fingerprint"
REMOTE_SCRIPT
