#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
ssh_target=${MAKO_PROXMOX_SSH:-root@localhost}
nft_source="$repository_root/infra/proxmox/public-beta/firewall/mako-vm124.nft"
unit_source="$repository_root/infra/proxmox/public-beta/firewall/mako-vm124-firewall.service"
remote_nft=/etc/nftables.d/mako-vm124.nft
remote_unit=/etc/systemd/system/mako-vm124-firewall.service

ssh -o BatchMode=yes "$ssh_target" -- /usr/sbin/nft --check --file /dev/stdin <"$nft_source"
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/install -d -o root -g root -m 0755 /etc/nftables.d
scp -q -o BatchMode=yes "$nft_source" "$ssh_target:$remote_nft.pending"
scp -q -o BatchMode=yes "$unit_source" "$ssh_target:$remote_unit.pending"
ssh -o BatchMode=yes "$ssh_target" -- \
  /usr/bin/install -o root -g root -m 0644 "$remote_nft.pending" "$remote_nft"
ssh -o BatchMode=yes "$ssh_target" -- \
  /usr/bin/install -o root -g root -m 0644 "$remote_unit.pending" "$remote_unit"
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/rm -f "$remote_nft.pending" "$remote_unit.pending"
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/systemctl daemon-reload
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/systemctl enable --now mako-vm124-firewall.service
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/systemctl restart mako-vm124-firewall.service
