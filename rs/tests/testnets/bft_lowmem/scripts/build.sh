#!/usr/bin/env bash
# Build the bft_lowmem local system test driver (boots the published mainnet dev
# GuestOS, so no image build) in the IC dev container, then write a standalone run
# script so the Bazel server can be shut down before VMs boot. Run without a TTY:
# with one, podman forwards the pty and stalled the build once.
set -x
unset SSH_AUTH_SOCK
cd /mnt/build/ic
BZ="--config=local --jobs=32 --local_resources=memory=HOST_RAM*.7 --local_resources=cpu=38 --curses=no --color=no --show_progress_rate_limit=60"
./ci/container/container-run.sh -c /mnt/build/cache bash -c "
  set -x
  bazel --host_jvm_args=-Xmx6g build $BZ //rs/tests/testnets:bft_lowmem_local &&
  bazel --host_jvm_args=-Xmx6g run $BZ --script_path=/ic/run_bft_lowmem.sh //rs/tests/testnets:bft_lowmem_local
  rc=\$?
  bazel shutdown
  echo BUILD_EXIT=\$rc
"
