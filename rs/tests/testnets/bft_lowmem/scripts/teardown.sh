#!/usr/bin/env bash
# Tear down a bft_lowmem testnet: stop the driver container, then any QEMU VMs,
# group dnsmasq, the host RSS sampler and the group bridge/TAP links it left.
for c in $(timeout 30 sudo podman ps -q); do timeout 90 sudo podman stop -t 30 "$c" >/dev/null; done
pgrep -f "[q]emu-system-x86_64" | xargs -r kill; sleep 5
pgrep -f "[q]emu-system-x86_64" | xargs -r kill -9
pgrep -f "[d]nsmasq.*vbr-" | xargs -r kill
pgrep -f "[m]easure.sh host" | xargs -r kill
for l in $(ip -o link show | grep -oE "(vbr|tap|ta4)-[0-9a-f]{10}" | sort -u); do sudo ip link del "$l" 2>/dev/null; done
echo "qemu=$(pgrep -c -f "[q]emu-system-x86_64") links=$(ip -o link | grep -c -E "vbr-|tap-") used_mib=$(free -m | awk "/^Mem/{print \$3}")"
