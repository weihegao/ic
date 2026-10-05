// Bootstrap step that icp-cli cannot do on a self-hosted testnet: create one canister
// through the management canister's provisional API (allowed because ic-prep sets the
// provisional whitelist to everyone), controlled by the icp-cli identity. icp-cli then
// installs its own proxy canister into it and does everything else through `--proxy`.
//
//   node bootstrap_proxy.mjs <host> <root-key-hex> <effective-canister-id> <controller> <cycles>
import { HttpAgent, Actor } from "@icp-sdk/core/agent";
import { IDL } from "@icp-sdk/core/candid";
import { Principal } from "@icp-sdk/core/principal";

const [host, rootKeyHex, effective, controller, cycles] = process.argv.slice(2);
const agent = await HttpAgent.create({
  host,
  rootKey: Uint8Array.from(Buffer.from(rootKeyHex, "hex")),
  shouldFetchRootKey: false,
});

const settings = IDL.Record({
  controllers: IDL.Opt(IDL.Vec(IDL.Principal)),
  compute_allocation: IDL.Opt(IDL.Nat),
  memory_allocation: IDL.Opt(IDL.Nat),
  freezing_threshold: IDL.Opt(IDL.Nat),
});
const idl = ({ IDL: _ }) =>
  IDL.Service({
    provisional_create_canister_with_cycles: IDL.Func(
      [
        IDL.Record({
          amount: IDL.Opt(IDL.Nat),
          settings: IDL.Opt(settings),
          specified_id: IDL.Opt(IDL.Principal),
        }),
      ],
      [IDL.Record({ canister_id: IDL.Principal })],
      [],
    ),
  });

const mgmt = Actor.createActor(idl, {
  agent,
  canisterId: Principal.fromText("aaaaa-aa"),
  effectiveCanisterId: Principal.fromText(effective),
});
const { canister_id } = await mgmt.provisional_create_canister_with_cycles({
  amount: [BigInt(cycles)],
  settings: [
    {
      controllers: [[Principal.fromText(controller)]],
      compute_allocation: [],
      memory_allocation: [],
      freezing_threshold: [],
    },
  ],
  specified_id: [],
});
console.log(canister_id.toText());
