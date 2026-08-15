#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
ssh_target=${MAKO_PROXMOX_SSH:-root@localhost}
guest_target=${MAKO_PUBLIC_BETA_SSH:-mako-admin@130.245.173.11}
known_hosts=${MAKO_PUBLIC_BETA_KNOWN_HOSTS:-$repository_root/.local/ansible/public-beta-known-hosts}
asset_root="$repository_root/infra/proxmox/public-beta/firewall"

ssh -o BatchMode=yes -o UserKnownHostsFile="$known_hosts" "$guest_target" -- \
  sudo /usr/bin/systemctl disable --now caddy.service

scp -q -o BatchMode=yes \
  "$asset_root/mako-vm124-admission-stop.nft" \
  "$ssh_target:/etc/nftables.d/mako-vm124-admission-stop.nft.pending"
scp -q -o BatchMode=yes \
  "$asset_root/mako-public-beta-admission-stop" \
  "$ssh_target:/usr/local/sbin/mako-public-beta-admission-stop.pending"
scp -q -o BatchMode=yes \
  "$asset_root/mako-vm124-admission-stop.service" \
  "$ssh_target:/etc/systemd/system/mako-vm124-admission-stop.service.pending"

ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/install -o root -g root -m 0644 \
  /etc/nftables.d/mako-vm124-admission-stop.nft.pending \
  /etc/nftables.d/mako-vm124-admission-stop.nft
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/install -o root -g root -m 0755 \
  /usr/local/sbin/mako-public-beta-admission-stop.pending \
  /usr/local/sbin/mako-public-beta-admission-stop
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/install -o root -g root -m 0644 \
  /etc/systemd/system/mako-vm124-admission-stop.service.pending \
  /etc/systemd/system/mako-vm124-admission-stop.service
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/rm -f \
  /etc/nftables.d/mako-vm124-admission-stop.nft.pending \
  /usr/local/sbin/mako-public-beta-admission-stop.pending \
  /etc/systemd/system/mako-vm124-admission-stop.service.pending
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/systemctl daemon-reload
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/systemctl enable mako-vm124-admission-stop.service
ssh -o BatchMode=yes "$ssh_target" -- /usr/bin/systemctl restart mako-vm124-admission-stop.service
ssh -o BatchMode=yes "$ssh_target" -- /usr/sbin/nft list table bridge mako_vm124_admission_stop >/dev/null

caddy_active=$(ssh -o BatchMode=yes -o UserKnownHostsFile="$known_hosts" "$guest_target" -- \
  sudo /usr/bin/systemctl is-active caddy.service || true)
caddy_enabled=$(ssh -o BatchMode=yes -o UserKnownHostsFile="$known_hosts" "$guest_target" -- \
  sudo /usr/bin/systemctl is-enabled caddy.service || true)
[[ "$caddy_active" == inactive && "$caddy_enabled" == disabled ]]
ssh -o BatchMode=yes -o UserKnownHostsFile="$known_hosts" "$guest_target" -- \
  sudo /usr/bin/systemctl is-active \
  mako-data-plane.service mako-control-plane.service mako-edge-gateway.service >/dev/null

echo "public-beta application admission stopped at Caddy and the Proxmox edge; VM, SSH, disks, backups, and evidence were retained"
