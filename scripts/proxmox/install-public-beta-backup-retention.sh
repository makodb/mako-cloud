#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
asset_root="$repository_root/infra/proxmox/public-beta/backup"

ssh -o BatchMode=yes root@localhost /usr/bin/install -o root -g root -m 0755 \
  "$asset_root/mako-public-beta-backup-retention" \
  /usr/local/sbin/mako-public-beta-backup-retention
ssh -o BatchMode=yes root@localhost /usr/bin/install -o root -g root -m 0644 \
  "$asset_root/mako-public-beta-backup-retention.service" \
  /etc/systemd/system/mako-public-beta-backup-retention.service
ssh -o BatchMode=yes root@localhost /usr/bin/install -o root -g root -m 0644 \
  "$asset_root/mako-public-beta-backup-retention.timer" \
  /etc/systemd/system/mako-public-beta-backup-retention.timer
ssh -o BatchMode=yes root@localhost /bin/systemctl daemon-reload
ssh -o BatchMode=yes root@localhost /bin/systemctl enable --now mako-public-beta-backup-retention.timer
ssh -o BatchMode=yes root@localhost /bin/systemctl start mako-public-beta-backup-retention.service

echo "installed off-VM public-beta backup retention timer"
