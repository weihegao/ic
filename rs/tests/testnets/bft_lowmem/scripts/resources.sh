#!/usr/bin/env bash
# Resource footprint of a running bft_lowmem testnet on this host, over a window.
#   resources.sh [window_s]
w=${1:-60}
hz=$(getconf CLK_TCK)
declare -A t0 name
for pid in $(pgrep -f "[q]emu-system-x86_64"); do
  name[$pid]=$(tr "\0" "\n" < /proc/$pid/cmdline | grep -m1 -oP "^guest=\K.*" | grep -oE "[a-z0-9]{5}-[a-z0-9]{3}$")
  read -r ut st < <(awk "{print \$14, \$15}" /proc/$pid/stat); t0[$pid]=$((ut+st))
done
br=$(ip -o link show | grep -oE "vbr-[0-9a-f]{10}" | head -1)
rx0=$(cat /sys/class/net/$br/statistics/rx_bytes); tx0=$(cat /sys/class/net/$br/statistics/tx_bytes)
read -r cpu0 < <(awk "/^cpu /{s=0; for(i=2;i<=NF;i++) s+=\$i; print s-\$5-\$6}" /proc/stat)
sleep $w
read -r cpu1 < <(awk "/^cpu /{s=0; for(i=2;i<=NF;i++) s+=\$i; print s-\$5-\$6}" /proc/stat)
rx1=$(cat /sys/class/net/$br/statistics/rx_bytes); tx1=$(cat /sys/class/net/$br/statistics/tx_bytes)
echo "vm,cpu_cores,rss_mib,disk_mib"
tot=0
for pid in "${!t0[@]}"; do
  read -r ut st < <(awk "{print \$14, \$15}" /proc/$pid/stat)
  cores=$(echo "scale=2; ($ut+$st-${t0[$pid]})/$hz/$w" | bc)
  rss=$(awk "/^VmRSS/{print int(\$2/1024)}" /proc/$pid/status)
  disk=$(tr "\0" "\n" < /proc/$pid/cmdline | grep -oP "file=\K[^,]*primary.qcow2" | xargs -r du -m --apparent-size 2>/dev/null | awk "{print \$1}")
  dreal=$(tr "\0" "\n" < /proc/$pid/cmdline | grep -oP "file=\K[^,]*primary.qcow2" | xargs -r du -m 2>/dev/null | awk "{print \$1}")
  echo "${name[$pid]},$cores,$rss,$dreal"
done | sort
echo "host_cpu_busy_cores=$(echo "scale=2; ($cpu1-$cpu0)/$hz/$w" | bc) of $(nproc)"
echo "loadavg=$(cut -d" " -f1-3 /proc/loadavg)"
free -m | awk "/^Mem/{print \"host_mem_used_mib=\"\$3\" avail_mib=\"\$7\" total_mib=\"\$2}"
echo "bridge_rx_mbit=$(echo "scale=2; ($rx1-$rx0)*8/$w/1000000" | bc) bridge_tx_mbit=$(echo "scale=2; ($tx1-$tx0)*8/$w/1000000" | bc)"
lb=$(dirname $(dirname $(tr "\0" "\n" < /proc/$(pgrep -f "[q]emu-system-x86_64" | head -1)/cmdline | grep -oP "file=\K[^,]*primary.qcow2")))
echo "base_image_mib=$(du -m $(dirname $lb)/image_cache/*.img 2>/dev/null | awk "{s+=\$1} END{print s}") vms_dir_mib=$(du -sm $lb | cut -f1)"
df -h /mnt/build | tail -1
