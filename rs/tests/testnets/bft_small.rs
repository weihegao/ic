// Real BFT demo: 1-node NNS + 4-node Application subnet (no boundary/gateway yet).
// Nodes sized to 4 GiB RAM (validated floor from network_large_test.rs), default 6 vCPU.
//
//   $ ./ci/container/container-run.sh
//   $ bazel run --config=local //rs/tests/testnets:bft_small_local -- --keepalive

use anyhow::Result;
use ic_consensus_system_test_utils::rw_message::install_nns_with_customizations_and_check_progress;
use ic_registry_subnet_type::SubnetType;
use ic_system_test_driver::driver::{
    group::SystemTestGroup,
    ic::{AmountOfMemoryKiB, InternetComputer, Subnet, VmResourceOverrides},
    test_env::TestEnv,
    test_env_api::{HasTopologySnapshot, NnsCustomizations},
};

fn main() -> Result<()> {
    SystemTestGroup::new()
        .with_setup(setup)
        .execute_from_args()?;
    Ok(())
}

pub fn setup(env: TestEnv) {
    InternetComputer::new()
        .with_resource_overrides(VmResourceOverrides {
            memory_kibibytes: Some(AmountOfMemoryKiB::new(4_195_000)), // 4 GiB
            ..VmResourceOverrides::default()
        })
        .add_subnet(Subnet::new(SubnetType::System).add_nodes(1))
        .add_subnet(Subnet::new(SubnetType::Application).add_nodes(4))
        .setup_and_start(&env)
        .expect("Failed to setup IC under test");
    install_nns_with_customizations_and_check_progress(
        env.topology_snapshot(),
        NnsCustomizations::default(),
    );
}
