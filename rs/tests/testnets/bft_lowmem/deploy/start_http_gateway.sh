#!/usr/bin/env bash
# Serve canister frontends from a self-hosted testnet in a browser.
#
# The replica's public endpoint does not serve http_request assets, so start a local
# PocketIC server and ask it for an HTTP gateway that forwards to the testnet node.
# Then open http://<canister-id>.localhost:<gateway-port>/
#
#   start_http_gateway.sh <replica-url> [gateway-port] [pocketic-server-port]
#   e.g. start_http_gateway.sh http://127.0.0.1:18090 18300 18200
#
# POCKET_IC defaults to the pocket-ic shipped with icp-cli's network launcher.
set -euo pipefail
replica=${1:?replica url}; gw_port=${2:-18300}; pic_port=${3:-18200}
POCKET_IC=${POCKET_IC:-$(ls -d "$HOME/Library/Application Support/org.dfinity.icp-cli/pkg/network-launcher"/*/pocket-ic 2>/dev/null | tail -1)}
nohup "$POCKET_IC" --port "$pic_port" --ttl 86400 > "pocket-ic-gateway.log" 2>&1 &
for _ in $(seq 1 20); do curl -sf "http://127.0.0.1:$pic_port/status" >/dev/null && break; sleep 0.5; done
curl -sf -X POST "http://127.0.0.1:$pic_port/http_gateway" -H 'content-type: application/json' \
  -d "{\"ip_addr\":\"127.0.0.1\",\"port\":$gw_port,\"forward_to\":{\"Replica\":\"$replica\"},\"domains\":[\"localhost\"],\"https_config\":null,\"domain_custom_provider_local_file\":null}"
echo
