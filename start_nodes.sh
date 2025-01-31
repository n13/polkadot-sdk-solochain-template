#!/bin/zsh

# Kill any previous solochain-template-node processes
pkill -f solochain-template-node

# Clean up old chain data
rm -rf /tmp/validator1 /tmp/validator2 /tmp/listener

# -----------------------------
# 1) Start Validator1
#    WebSocket on 127.0.0.1:9944
# -----------------------------
./target/release/solochain-template-node \
  --base-path /tmp/validator1 \
  --chain custom-spec-raw.json \
  --port 30333 \
  --validator \
  --name Validator1 \
  --experimental-rpc-endpoint "listen-addr=127.0.0.1:9944,methods=unsafe,cors=all" \
  --node-key cffac33ca656d18f3ae94393d01fe03d6f9e8bf04106870f489acc028b214b15 \
  &

# Wait for Validator1 to come online
sleep 5

# Retrieve its peer ID via HTTP on the same endpoint
VALIDATOR_1_PEER_ID=$(
  curl -s http://127.0.0.1:9944 \
    -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"system_localPeerId","id":1}' \
  | jq -r '.result'
)

if [ -z "$VALIDATOR_1_PEER_ID" ] || [ "$VALIDATOR_1_PEER_ID" = "null" ]; then
  echo "Failed to retrieve Validator1 Peer ID"
  exit 1
fi

echo "Validator1 Peer ID: $VALIDATOR_1_PEER_ID"

# -----------------------------
# 2) Start Validator2
#    WebSocket on 127.0.0.1:9945
# -----------------------------
./target/release/solochain-template-node \
  --base-path /tmp/validator2 \
  --chain custom-spec-raw.json \
  --port 30334 \
  --validator \
  --name Validator2 \
  --node-key bbb5338fe3dbe14aacde7465aac6606ce22a9630ad63978030224764d6fb2c51 \
  --experimental-rpc-endpoint "listen-addr=127.0.0.1:9945,methods=unsafe,cors=all" \
  --bootnodes /ip4/127.0.0.1/tcp/30333/p2p/$VALIDATOR_1_PEER_ID \
  &

# -----------------------------
# 3) Start Listener (non-validator)
#    WebSocket on 127.0.0.1:9946
# -----------------------------
./target/release/solochain-template-node \
  --base-path /tmp/listener \
  --chain custom-spec-raw.json \
  --port 30335 \
  --name Listener \
  --experimental-rpc-endpoint "listen-addr=127.0.0.1:9946,methods=unsafe,cors=all" \
  --bootnodes /ip4/127.0.0.1/tcp/30333/p2p/$VALIDATOR_1_PEER_ID \
  &