# this is needed so we have a chain spec to read from

# first build the project then run this

cargo build --release

./target/release/solochain-template-node \
  build-spec \
  --chain dev \
  --disable-default-bootnode > custom-spec.json

./target/release/solochain-template-node \
  build-spec \
  --chain custom-spec.json \
  --raw \
  --disable-default-bootnode > custom-spec-raw.json