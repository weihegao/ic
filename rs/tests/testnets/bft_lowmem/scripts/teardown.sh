#!/usr/bin/env bash
# Tear down a bft_lowmem testnet: stop the driver container, then any QEMU VMs,
# group dnsmasq, the host RSS sampler and the group bridge/VXLAN/TAP links it left,
# here and on every remote host listed (one ssh target per line) in /mnt/build/hosts.txt.
for c in $(timeout 30 sudo podman ps -q); do timeout 90 sudo podman stop -t 30 "$c" >/dev/null; done
pgrep -f "[q]emu-system-x86_64" | xargs -r kill; sleep 5
pgrep -f "[q]emu-system-x86_64" | xargs -r kill -9
pgrep -f "[d]nsmasq.*vbr-" | xargs -r kill
pgrep -f "[m]easure.sh host" | xargs -r kill
for l in $(ip -o link show | grep -oE "(vbr|tap|ta4|vx)-[0-9a-f]{10}" | sort -u); do sudo ip link del "$l" 2>/dev/null; done
echo "local: qemu=$(pgrep -c -f "[q]emu-system-x86_64") links=$(ip -o link | grep -c -E "vbr-|tap-|vx-") used_mib=$(free -m | awk "/^Mem/{print \$3}")"
[ -f /mnt/build/hosts.txt ] && while read -r h; do
  [ -z "$h" ] && continue
  ssh -n -o BatchMode=yes -o ConnectTimeout=10 "$h" 'pgrep -f "[q]emu-system-x86_64" | xargs -r kill; sleep 3; pgrep -f "[q]emu-system-x86_64" | xargs -r kill -9
for l in $(ip -o link show | grep -oE "(vbr|tap|ta4|vx)-[0-9a-f]{10}" | sort -u); do sudo -n ip link del "$l" 2>/dev/null; done
for d in /var/tmp/ictest/vbr-*; do [ -d "$d" ] && rm -rf "$d"; done
echo "$(hostname): qemu=$(pgrep -c -f "[q]emu-system-x86_64") links=$(ip -o link | grep -c -E "vbr-|tap-|vx-") used_mib=$(free -m | awk "/^Mem/{print \$3}")"'
done < /mnt/build/hosts.txt
