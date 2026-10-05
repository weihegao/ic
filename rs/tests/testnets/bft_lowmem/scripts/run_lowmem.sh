#!/usr/bin/env bash
# Boot one bft_lowmem testnet (--keepalive) and sample per-VM host RSS while it runs.
#   run_lowmem.sh <label> [BFT_VAR=value ...]
# Logs: /mnt/build/runs/<label>/{run.log,host_rss.csv}
label=${1:?label}; shift
out=/mnt/build/runs/$label; mkdir -p "$out"
cd /mnt/build/ic; unset SSH_AUTH_SOCK
/mnt/build/measure.sh host "$out/host_rss.csv" 10 & MP=$!
echo "$label $*" > "$out/params.txt"
EXECROOT=$(grep -oP "^cd \\K\\S+" /mnt/build/ic/run_bft_lowmem.sh)
TMP=$EXECROOT/$(grep -oP "TEST_TMPDIR=\\K\\S+" /mnt/build/ic/run_bft_lowmem.sh)
echo "TEST_TMPDIR=$TMP" >> "$out/params.txt"
./ci/container/container-run.sh -c /mnt/build/cache bash -c "rm -rf $TMP; export $*; /ic/run_bft_lowmem.sh --keepalive" </dev/null > "$out/run.log" 2>&1
echo "RUN_EXIT=$?" >> "$out/run.log"
kill $MP
