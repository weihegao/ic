// Low-memory BFT testnet: one N-node System subnet (the root/NNS subnet), plus an
// optional Application subnet, with every node VM sized well below the 4 GiB floor
// used upstream. Used to find how little RAM a real GuestOS IC node needs.
//
// All knobs come from the environment (pass them with `--test_env=NAME=value`):
//   BFT_NODES                nodes in the System subnet            (default 4)
//   BFT_APP_NODES            nodes per Application subnet          (default 0 = none)
//   BFT_APP_SUBNETS          number of Application subnets         (default 1)
//   BFT_MEM_MIB              RAM per node VM, MiB                   (default 2048)
//   BFT_VCPUS                vCPUs per node VM                      (default 2)
//   BFT_DKG_INTERVAL         DKG interval = checkpoint interval     (default 49)
//   BFT_MAX_STATE_DELTA_MIB  registry maximum_state_delta, MiB      (default 512)
//   BFT_INSTALL_NNS          1 = install the NNS canisters          (default 0)
//   BFT_SOAK_UPDATES         update calls the test makes            (default 50)
//
// The DKG interval and maximum_state_delta are registry settings, so they need no
// IC code change: a short interval checkpoints (and flushes heap delta out of RAM)
// more often, and a small maximum_state_delta also lowers the sandbox RSS limit.
//
//   $ ./ci/container/container-run.sh
//   $ bazel run --config=local //rs/tests/testnets:bft_lowmem_local \
//       --test_env=BFT_MEM_MIB=2048 -- --keepalive

use anyhow::Result;
use ic_consensus_system_test_utils::rw_message::{
    can_read_msg_with_retries, cert_state_makes_progress_with_retries,
    install_nns_with_customizations_and_check_progress, store_message,
};
use ic_registry_resource_limits::ResourceLimits;
use ic_registry_subnet_type::SubnetType;
use ic_system_test_driver::driver::{
    group::SystemTestGroup,
    ic::{AmountOfMemoryKiB, InternetComputer, NrOfVCPUs, Subnet, VmResourceOverrides},
    test_env::TestEnv,
    test_env_api::{
        HasPublicApiUrl, HasTopologySnapshot, IcNodeContainer, NnsCustomizations, secs,
    },
};
use ic_system_test_driver::util::{MessageCanister, assert_create_agent};
use ic_system_test_driver::{systest, util::block_on};
use ic_types::{Height, NumBytes};
use slog::info;
use std::time::{Duration, Instant};

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(v) => v
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}={v:?} is not a non-negative integer")),
        Err(_) => default,
    }
}

fn main() -> Result<()> {
    SystemTestGroup::new()
        .with_setup(setup)
        .add_test(systest!(progress))
        .with_timeout_per_test(Duration::from_secs(60 * 60))
        .with_overall_timeout(Duration::from_secs(4 * 60 * 60))
        .execute_from_args()?;
    Ok(())
}

pub fn setup(env: TestEnv) {
    let log = env.logger();
    let nodes = env_u64("BFT_NODES", 4) as usize;
    let app_nodes = env_u64("BFT_APP_NODES", 0) as usize;
    let app_subnets = env_u64("BFT_APP_SUBNETS", 1) as usize;
    let mem_mib = env_u64("BFT_MEM_MIB", 2048);
    let vcpus = env_u64("BFT_VCPUS", 2);
    let dkg = env_u64("BFT_DKG_INTERVAL", 49);
    let delta_mib = env_u64("BFT_MAX_STATE_DELTA_MIB", 512);
    let install_nns = env_u64("BFT_INSTALL_NNS", 0) == 1;
    info!(
        log,
        "bft_lowmem: system={nodes} app={app_subnets}x{app_nodes} mem={mem_mib}MiB vcpus={vcpus} \
         dkg={dkg} max_state_delta={delta_mib}MiB install_nns={install_nns}"
    );

    let limits = ResourceLimits {
        maximum_state_size: None,
        maximum_state_delta: Some(NumBytes::from(delta_mib * 1024 * 1024)),
    };
    let subnet = |subnet_type, n| {
        Subnet::new(subnet_type)
            .with_dkg_interval_length(Height::from(dkg))
            .with_resource_limits(limits)
            .add_nodes(n)
    };
    let mut ic = InternetComputer::new()
        .with_resource_overrides(VmResourceOverrides {
            vcpus: Some(NrOfVCPUs::new(vcpus)),
            memory_kibibytes: Some(AmountOfMemoryKiB::new(mem_mib * 1024)),
            ..VmResourceOverrides::default()
        })
        .add_subnet(subnet(SubnetType::System, nodes));
    if app_nodes > 0 {
        for _ in 0..app_subnets {
            ic = ic.add_subnet(subnet(SubnetType::Application, app_nodes));
        }
    }
    ic.setup_and_start(&env)
        .expect("Failed to setup IC under test");

    let start = Instant::now();
    for node in env.topology_snapshot().subnets().flat_map(|s| s.nodes()) {
        node.await_status_is_healthy()
            .expect("node did not become healthy");
    }
    info!(
        log,
        "bft_lowmem: all nodes healthy after {:?}",
        start.elapsed()
    );

    if install_nns {
        let start = Instant::now();
        install_nns_with_customizations_and_check_progress(
            env.topology_snapshot(),
            NnsCustomizations::default(),
        );
        info!(log, "bft_lowmem: NNS installed after {:?}", start.elapsed());
    }
}

/// Create a canister, make update calls through one node, read the result back from
/// every node of the subnet, and check certified state keeps advancing on each node.
pub fn progress(env: TestEnv) {
    let log = env.logger();
    let updates = env_u64("BFT_SOAK_UPDATES", 50);
    let topology = env.topology_snapshot();
    for subnet in topology.subnets() {
        let nodes: Vec<_> = subnet.nodes().collect();
        let first = &nodes[0];
        let url = first.get_public_url();
        let canister_id = store_message(&url, first.effective_canister_id(), "msg-0", &log);

        let start = Instant::now();
        block_on(async {
            let agent = assert_create_agent(url.as_str()).await;
            let mcan = MessageCanister::from_canister_id(&agent, canister_id);
            for i in 1..=updates {
                mcan.store_msg(format!("msg-{i}")).await;
            }
        });
        let elapsed = start.elapsed();
        info!(
            log,
            "bft_lowmem: subnet {} ({} nodes): {updates} updates in {elapsed:?} ({:.0} ms/update)",
            subnet.subnet_id,
            nodes.len(),
            elapsed.as_millis() as f64 / updates.max(1) as f64
        );

        let last = format!("msg-{updates}");
        for node in &nodes {
            assert!(
                can_read_msg_with_retries(&log, &node.get_public_url(), canister_id, &last, 10),
                "node {} cannot read the last message",
                node.node_id
            );
            cert_state_makes_progress_with_retries(
                &node.get_public_url(),
                node.effective_canister_id(),
                &log,
                secs(120),
                secs(5),
            );
        }
        info!(
            log,
            "bft_lowmem: subnet {} replicated + progressing", subnet.subnet_id
        );
    }
}
