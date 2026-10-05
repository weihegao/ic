# bft_lowmem: real IC GuestOS nodes at 1–2 GiB of RAM

This directory belongs to the `bft_lowmem` testnet (`../bft_lowmem.rs`, `../BUILD.bazel`).
It holds the scripts, measurements and an icp-cli deploy kit showing that a real, multi-node
IC — 1 NNS node plus a **13-node BFT application subnet** — runs on GuestOS VMs with
**2 GiB of RAM each**, all on a single 40-vCPU / 32 GiB host, **with no change to IC code**.
It also shows that you can `icp deploy` a canister to that network from another machine.

## TL;DR

- **2 GiB per node works.** It works for a 4-node subnet with the full NNS installed (update
  latency ~1.1 s, similar to mainnet) and for 1 NNS node + a 13-node application subnet
  (14 VMs = 17.3 GiB of host RAM).
- **The floor is 1–1.5 GiB, and the limit is wasm compilation.** At 1 GiB each `compiler_sandbox`
  compiling an NNS canister needs 550–650 MiB, gets OOM-killed, and the NNS install never
  finishes. A node at rest needs only ~340 MiB.
- **Plan host RAM at the nominal size per node.** The virtio-balloon has no free-page reporting,
  so host RSS ratchets up to whatever the guest has touched (~1.9 GiB on the NNS node).
- **On one host, CPU runs out before RAM.** 28 guest vCPUs on 40 host vCPUs gave load ~63, which
  inflates the 13-node subnet's update latency to ~2.6 s.
- **icp-cli works against it** through its `--proxy` mechanism (details below).

## How the testnet is configured

`bft_lowmem.rs` boots one N-node System subnet (the NNS subnet) plus an optional Application
subnet. Everything is set from the environment (`--test_env=…`, or exported before the run script):

| Variable | Meaning | Default |
|---|---|---|
| `BFT_NODES` | nodes in the System (NNS) subnet | 4 |
| `BFT_APP_NODES` | nodes in an extra Application subnet (0 = none) | 0 |
| `BFT_MEM_MIB` | RAM per node VM | 2048 |
| `BFT_VCPUS` | vCPUs per node VM | 2 |
| `BFT_DKG_INTERVAL` | DKG interval, which is also the checkpoint interval | 49 |
| `BFT_MAX_STATE_DELTA_MIB` | registry `maximum_state_delta` | 512 |
| `BFT_INSTALL_NNS` | 1 = install the NNS canisters | 0 |
| `BFT_SOAK_UPDATES` | update calls the `progress` test makes | 50 |

- The DKG interval and `maximum_state_delta` are subnet-record (registry) settings, so no IC code
  changes. A short interval checkpoints, and so flushes heap delta out of RAM, more often. A small
  `maximum_state_delta` also lowers the sandbox RSS limit (it is `maximum_state_delta / 3`).
- The target boots the **published mainnet dev GuestOS** (`guestos = "mainnet_latest_dev"`, currently
  `feb8e5f2`) and the mainnet NNS canisters, so only the Rust test driver is compiled. No GuestOS
  image is built.
- The `progress` test makes `BFT_SOAK_UPDATES` sequential update calls through one node of each
  subnet. It then checks that every node of that subnet reads the last value back and keeps
  advancing its certified state.

## Running it (local QEMU backend, one Linux host with KVM)

`scripts/` holds what was used on hera (dept OpenStack). The host was VM `ic-lowmem`
(`flv_dnet79_c40_m32768`, 40 vCPU / 31 GiB, nested KVM) with a 200 GB volume at `/mnt/build`
that holds this repo clone (`/mnt/build/ic`), the Bazel cache and the run logs.

```bash
scripts/build.sh      # build the driver in the dev container; write /ic/run_bft_lowmem.sh; stop bazel
scripts/run_lowmem.sh m2048-nns1-app13 BFT_NODES=1 BFT_APP_NODES=13 BFT_MEM_MIB=2048 BFT_INSTALL_NNS=1
scripts/measure.sh guests /mnt/build/runs/<label>/run.log /mnt/build/runs/<label>/guests  # per-process RSS in every node
scripts/teardown.sh   # stop the driver container, QEMU VMs, dnsmasq, the sampler and the bridge/TAPs
```

`run_lowmem.sh` keeps the testnet up (`--keepalive`) and samples every node VM's host RSS every 10 s.
Lessons learned getting there:

- **Run the dev container without a TTY** (`setsid nohup … </dev/null`). Under tmux, `container-run.sh`
  adds `-i -t`, podman's pty forwarding spun at 180% CPU, and Bazel stalled writing progress output.
- **Unset `SSH_AUTH_SOCK`** for detached runs. The script bind-mounts the agent socket, which
  disappears when the SSH session that started the job closes.
- **A killed build can corrupt the zig cache** (`/tmp/zig-cache` → `<cache>/zig-cache`).
  Symptom: "C compiler cannot create executables" with `ld.lld: cannot open …/conftest.o`.
  Fix: delete the zig cache.
- Keep the Bazel cache off a small root disk. The first fetch alone is ~17 GB.

## Results (hera, 2026-10-05)

**Measurements.**
- *Guest* numbers are `free -m` inside each node after the test. *used* excludes page cache; *available* is what the kernel could still hand out.
- *Host RSS* is the QEMU process's resident memory, i.e. what the node really costs the host.
- Raw data: `data/<run>/` (`params.txt`, `summary.log`, `host_rss.csv` sampled every 10 s, and `guests/*.txt` per-process snapshots).

| Run | RAM/node | Topology | Healthy | NNS install | Update latency | Guest used / avail | Host RSS/VM |
|---|---|---|---|---|---|---|---|
| `m2048-n4-b` | 2 GiB | 4-node subnet, no NNS | 55 s | — | 1,110 ms (200 updates) | ~615 / 1,340 MiB | ~1.06 GiB |
| `m2048-n4-nns` | 2 GiB | 4-node subnet + NNS | 50 s | 37 s | 1,125 ms | ~670 / 1,280 MiB | 1.85–1.91 GiB |
| `m1536-n4-nns` | 1.5 GiB | 4-node subnet + NNS | 55 s | 40 s | 1,672 ms | ~610 / 840 MiB | = nominal (1.5 GiB) |
| `m1024-n4-nns` | 1 GiB | 4-node subnet + NNS | yes | **failed** (300 s timeout, 145 OOM kills) | — | ~345 / 600 MiB | — |
| `m2048-nns1-app13` | 2 GiB | 1 NNS node + **13-node** app subnet | 158 s | 89 s | NNS 932 ms; **13-node 2,589 ms** | app ~710 / 1,240 MiB; NNS 590 / 1,360 MiB | app ~1.19 GiB; NNS 1.80 GiB; **14 VMs = 17.3 GiB** |

Per-process RSS in a 2 GiB node:

| Process | 4-node subnet | 13-node subnet |
|---|---|---|
| replica | 190–230 MiB | ~330 MiB |
| ic-crypto-csp | ~78 MiB | ~78 MiB |
| canister sandboxes | ~14 MiB per canister; 11 NNS canisters ≈ 187 MiB | same |
| everything else (systemd, journald, danted ×20, 4 btc/doge adapters, node_exporter, orchestrator) | ~250 MiB | ~250 MiB |

The replica is bigger on the 13-node subnet because consensus and P2P state grow with the number of peers.

### What this shows

1. **2 GiB per node works with no IC code change.**
   - A 4-node BFT subnet with the full NNS installed works at 2 GiB per node.
   - So does a 1 NNS + 13-node Application subnet.
   - Update latency on the 4-node subnet (~1.1 s) matches IC mainnet (~1.2–1.3 s).
2. **The real floor is 1–1.5 GiB, and the limit is wasm compilation, not consensus.**
   - At 1 GiB, steady state was only ~340 MiB used.
   - But every `compiler_sandbox` compiling an NNS canister reached 550–650 MiB of anonymous memory, was OOM-killed, and retried until the install timed out.
   - At 1.5 GiB the install succeeds, but updates are ~50% slower, probably because page cache gets squeezed.
   - Getting to 1 GiB would need the Tier 1 compile fixes: fewer compile threads (`num_rayon_compilation_threads`) and a limit on concurrent compilations.
3. **Plan host RAM at the nominal size per node, not at "used".**
   - QEMU allocates guest RAM lazily, but the virtio-balloon has no free-page reporting, so nothing the guest touches is returned.
   - A node that installed the NNS settled at ~1.85–1.9 GiB of host RSS, close to the full 2 GiB. Application nodes, which never compile the NNS, stayed around 1.2 GiB.
   - Turning on `free-page-reporting` on the balloon in `local_backend.rs` would let hosts reclaim guest free memory. Page cache would still count.
4. **Per-node sizing can differ by role.** Only the node(s) that compile large canisters (the NNS subnet) need 2 GiB. Application-subnet nodes running small canisters could likely use 1–1.5 GiB. This was not run here.
5. **The single-host cap on hera is CPU, not RAM.**
   - The 14-node network used 17.3 GiB of 31 GiB.
   - CPU, however, was oversubscribed: 28 guest vCPUs plus the host, load average ~63 on 40 vCPU.
   - That is why the 13-node subnet's latency (2.6 s) is inflated. These numbers show correctness and memory footprint, not IC performance.

### Caveats

## Deploying a canister with icp-cli (proven 2026-10-05)

Goal: use the normal icp-cli workflow, just with a different endpoint. The testnet came from
`m2048-nns1-app13` (1 NNS node + 13-node application subnet, 2 GiB per node); icp-cli 1.0.2 ran on
a separate machine (macOS) on the hera intranet.

1. **Reach a node.** The nodes only have ULA IPv6 addresses on the testnet host's bridge, so forward
   one application node's HTTP port:
   `ssh -N -L '18090:[<app-node-ipv6>]:8080' ubuntu@<host>`. Node URLs appear in the run log as
   `http://[fd00:…]:8080`. `icp network ping` then reports `healthy`, `impl_version feb8e5f2…` and
   the certified height.
2. **Root key.** Take the NNS public key PEM (`<TEST_TMPDIR>/tests/progress/ic_prep/nns_public_key.pem`)
   and convert it to hex DER (133 bytes). Put it in `deploy/icp.yaml` as a `connected` network, or
   pass `-n <url> -k <hex>`.
3. **Why plain `icp deploy` fails, and the workaround.**
   - icp-cli creates canisters through the **cycles ledger** (`um5iw-rqaaa-aaaaq-qaaba-cai`), which a
     test NNS install does not include (error: `Canister um5iw-… not found`).
   - icp-cli 1.0.2 also cannot verify **subnet-scoped** responses from this replica version. With
     `--subnet` or a management-canister call it looks up `/subnet/<id>/canister_ranges` in the
     delegation, and that path is not in the certificate.
   - Canister-scoped calls verify fine. So we use icp-cli's `--proxy`: every management call is routed
     through a proxy canister's `proxy` method, which is canister-scoped.
4. **Bootstrap the proxy (the one non-icp-cli step).**
   - `node deploy/bootstrap_proxy.mjs http://127.0.0.1:18090 <root-key-hex> <effective-id> <your-principal> 100000000000000`
     creates a canister with 100T cycles through `provisional_create_canister_with_cycles`. This is
     allowed because ic-prep whitelists everyone. `<effective-id>` is any canister ID in the target
     subnet's range; the first one is logged as "Storing a message in canister with id …".
   - `deploy/extract_proxy_wasm.py` pulls icp-cli's own proxy wasm out of the `icp` binary
     (module hash `9e5abf27…`).
   - `icp canister install <proxy-id> --wasm proxy.wasm -n … -k …` installs it.
5. **Deploy and call:**
   ```
   $ icp deploy -e testnet --proxy 53zcu-tiaaa-aaaaa-qaaba-cai
   Created canister greet with ID 54yea-6qaaa-aaaaa-qaabq-cai
   $ icp canister call -e testnet greet greet_stable_update '("weihe from a self-hosted 13-node subnet")'
   ("Hello, weihe from a self-hosted 13-node subnet!")
   ```
6. **Replication check.** Open a second tunnel to a *different* application node, which never
   received the deploy or the update, and query it there:
   - `greet_query` answers from that node;
   - `icp canister status` through that node reports module hash `0x0bad1ab7…5db9`, which equals the
     SHA-256 of the wasm deployed from the other machine.

   So the install was replicated by consensus across the 13-node subnet.

### Full dApp with a browser frontend: `dfinity/icp-hello-world-rust`

This was deployed on the same 13-node subnet. It is a Rust `backend` (`greet` query) plus a Vite
`frontend` asset canister. The example is a **dfx** project, and icp-cli 1.0.2 can only upload assets
through a downloadable plugin, so dfx deploys it, into canisters that already exist:

1. **Create the two canisters with icp-cli through the proxy**, with the dfx identity as a controller:
   `icp canister create --detached --proxy <proxy> --controller <dfx-principal> --controller <icp-principal> --cycles 10t -n … -k …`
   gave `backend` = `5j7vn-7yaaa-aaaaa-qaaca-cai` and `frontend` = `5o6tz-saaaa-aaaaa-qaacq-cai`.
2. **Point dfx at the testnet.** Add `"networks": {"bft13": {"providers": ["http://127.0.0.1:18090"], "type": "persistent"}}`
   to `dfx.json`, and write `canister_ids.json` with those IDs under `bft13`.
3. **Deploy:**
   ```
   RUSTUP_TOOLCHAIN=1.93.1 dfx deploy --network bft13 --no-wallet --yes \
     --provisional-create-canister-effective-canister-id <first-id-in-the-subnet-range>
   ```
   - dfx fetches the root key from the node's `/api/v2/status`.
   - "All canisters have already been created", so there are no cycles-ledger calls.
   - It builds the backend and the Vite frontend, then installs both and uploads the 5 assets.
   - dfx also creates a Candid UI canister through the provisional API.
   - Without the effective-canister-id flag, dfx fails with "Subnet is not authorized to respond for
     the requested canister id".
4. **Browser.** The replica endpoint does not serve `http_request` assets, so
   `deploy/start_http_gateway.sh http://127.0.0.1:18090` starts a local PocketIC server whose HTTP gateway
   forwards to the testnet node (`forward_to: {"Replica": url}`).
   - `http://5o6tz-saaaa-aaaaa-qaacq-cai.localhost:18300/` serves the page with a valid `ic-certificate`.
   - Submitting a name returned "Hello, Weihe on our own 13-node subnet!" from the backend, through
     the gateway, on the 13-node subnet.

Notes:
- The deployed canister's controller is the proxy, since the proxy created it. Add your own principal
  as a controller (through the proxy) if you want to manage it directly.
- A newer icp-cli (1.6.0 is out) probably verifies subnet-scoped certificates and would not need the
  proxy for that reason. The cycles ledger would still be missing.

## Where the memory goes (code reading, before the runs)

- **Hard gates that do not apply.** SetupOS's 510 GiB check only runs in the bare-metal installer.
  The system-test default of 24 GiB per VM is just a default.
- **Replica constants are accounting ceilings, not allocations.** Examples: 2 TiB subnet memory,
  140 GiB heap delta, sandbox RSS limit = heap delta / 3, 64 GB LMDB map, 10 GiB on-disk
  compilation cache.
- **Sandboxing cannot be turned off by config.** `ExecutionEnvironment::new` asserts it is on
  (DTS needs it).
- **What would get a node to 1 GiB (code changes, not done):**
  - fewer `num_rayon_compilation_threads` (default 10), or a semaphore limiting concurrent compiles;
  - `SubnetConfig` is hard-coded (`setup_ic_stack.rs`), e.g. the 32 GiB heap-delta initial reserve
    and 4 scheduler cores;
  - `EmbeddersConfig` has no `serde(default)`, so it can only be overridden as a whole;
  - cap the ingress pool, which is **per peer** (100 MB each in the GuestOS template);
  - optionally mask always-on services: 4 btc/doge adapters, `danted` (~43 MiB across 20 processes),
    and remote attestation;
  - for upgrade tests below 2 GiB, move `manageboot`'s tar extraction out of `/tmp`, which is a
    RAM-backed tmpfs.
