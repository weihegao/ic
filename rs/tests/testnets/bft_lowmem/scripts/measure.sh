#!/usr/bin/env bash
# Memory measurement for a bft_lowmem local testnet, run on the host (hera main).
#
#   measure.sh host <out.csv> [interval_s]   sample per-VM QEMU RSS until killed
#   measure.sh guests <run.log> <outdir>     one per-process snapshot inside every node
#
# Host QEMU RSS = guest RAM the node has actually touched (+ small QEMU overhead):
# the real host cost of a node, since guest RAM is faulted in lazily.
set -uo pipefail
mode=${1:?mode}

if [ "$mode" = host ]; then
  out=${2:?out.csv}; iv=${3:-10}
  [ -s "$out" ] || echo "ts,vm,rss_mib,host_used_mib,host_avail_mib" > "$out"
  while true; do
    ts=$(date +%s)
    read -r used avail < <(free -m | awk '/^Mem/{print $3, $7}')
    for pid in $(pgrep -f qemu-system-x86_64); do
      vm=$(tr '\0' '\n' < /proc/$pid/cmdline 2>/dev/null | grep -m1 -oP '^guest=\K.*')
      rss=$(awk '/^VmRSS/{print int($2/1024)}' /proc/$pid/status 2>/dev/null)
      [ -n "$vm" ] && [ -n "$rss" ] && echo "$ts,$vm,$rss,$used,$avail" >> "$out"
    done
    sleep "$iv"
  done
fi

if [ "$mode" = guests ]; then
  log=${2:?run.log}; outdir=${3:?outdir}; mkdir -p "$outdir"
  key=$(find /mnt/build/cache /mnt/build/tmp -path '*ssh/authorized_priv_keys/admin' 2>/dev/null | head -1)
  [ -n "$key" ] || { echo "no admin key found"; exit 1; }
  ips=$(grep -oE 'fd00:[0-9a-f:]+' "$log" | sort -u | grep -vE '::1$|:1::1$|:2::1$|:3::1$')
  for ip in $ips; do
    f="$outdir/$(echo "$ip" | tr ':' '_').txt"
    timeout 30 ssh -i "$key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
      -o ConnectTimeout=8 -o BatchMode=yes "admin@$ip" '
        echo "== $(hostname) =="; date -u +%FT%TZ; uptime
        echo "== free -m =="; free -m
        echo "== meminfo =="; grep -E "MemTotal|MemAvailable|^Cached|Shmem:|AnonPages|Slab|SwapTotal|SwapFree" /proc/meminfo
        echo "== top RSS (KiB) =="; ps -eo rss=,pid=,args= --sort=-rss | head -30 | cut -c1-150
        echo "== RSS by command (MiB) =="
        ps -eo rss=,comm= | awk "{s[\$2]+=\$1; n[\$2]++} END{for(c in s) printf \"%8.1f %3d %s\n\", s[c]/1024, n[c], c}" | sort -rn | head -25
        echo "== replica jemalloc/process metrics =="
        curl -s --max-time 5 http://[::1]:9090/metrics 2>/dev/null | grep -E "^(process_resident_memory_bytes|jemalloc_(resident|allocated|active|mapped)_bytes) " || true
      ' > "$f" 2>&1 && echo "ok $ip" || echo "FAILED $ip (see $f)"
  done
fi
