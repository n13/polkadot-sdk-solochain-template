# Start Validator1 in the background
./target/release/solochain-template-node \
  --base-path /tmp/validator1 \
  --chain=custom-spec-raw.json \
  --port 30333 \
  --rpc-port 9933 \
  --rpc-external \
  --rpc-methods=Unsafe \
  --rpc-cors all \
  --name Validator1 \
  --node-key cffac33ca656d18f3ae94393d01fe03d6f9e8bf04106870f489acc028b214b15 \
  --public-addr /ip4/192.168.18.253/tcp/30333 \
  --no-telemetry &

# Wait for the node to start
sleep 5

# Retrieve the peer ID of Validator1 (retry mechanism)
RETRY_COUNT=0
VALIDATOR_1_PEER_ID=""
while [ -z "$VALIDATOR_1_PEER_ID" ] && [ $RETRY_COUNT -lt 10 ]; do
  VALIDATOR_1_PEER_ID=$(curl -s --header "Host: localhost" http://localhost:9933 -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"system_localPeerId","id":1}' | \
    jq -r '.result' 2>/dev/null)
  if [ -z "$VALIDATOR_1_PEER_ID" ] || [ "$VALIDATOR_1_PEER_ID" = "null" ]; then
    echo "Waiting for Validator1 peer ID... ($((RETRY_COUNT+1))/10)"
    sleep 2
    RETRY_COUNT=$((RETRY_COUNT+1))
  fi
done

if [ -z "$VALIDATOR_1_PEER_ID" ] || [ "$VALIDATOR_1_PEER_ID" = "null" ]; then
  echo "Failed to retrieve peer ID for Validator1"
  exit 1
fi

echo "Validator1 Peer ID: $VALIDATOR_1_PEER_ID"

# Start Validator2
./target/release/solochain-template-node \
  --base-path /tmp/validator2 \
  --chain=custom-spec-raw.json \
  --port 30334 \
  --rpc-port 9934 \
  --rpc-external \
  --rpc-methods=Unsafe \
  --rpc-cors all \
  --name Validator2 \
  --node-key bbb5338fe3dbe14aacde7465aac6606ce22a9630ad63978030224764d6fb2c51 \
  --public-addr /ip4/192.168.18.253/tcp/30334 \
  --no-telemetry \
  --bootnodes /ip4/127.0.0.1/tcp/30333/p2p/$VALIDATOR_1_PEER_ID &

# Start Listener
./target/release/solochain-template-node \
  --base-path /tmp/listener \
  --chain=custom-spec-raw.json \
  --port 30335 \
  --rpc-port 9935 \
  --rpc-external \
  --rpc-methods=Unsafe \
  --rpc-cors all \
  --name Listener \
  --bootnodes /ip4/127.0.0.1/tcp/30333/p2p/$VALIDATOR_1_PEER_ID &
