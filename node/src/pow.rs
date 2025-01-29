use sp_consensus_pow::{Error as PowError, PowAlgorithm};
use sp_runtime::{
    traits::{Header as HeaderT, NumberFor},
    DigestItem,
};
use sp_core::{U256, Encode};
use sha3::{Digest, Sha3_256};
use solochain_template_runtime::opaque::Block; // Adjust to your runtime's block type

/// A basic PoW algorithm that:
/// 1) Reads an 8-byte `nonce` from the block header's DigestItem
/// 2) Hashes `(parent_hash, block_number, nonce)`
/// 3) Checks if the result is < a fixed difficulty.
pub struct SimplePowAlgorithm;

impl<H> PowAlgorithm<H> for SimplePowAlgorithm
where
    H: HeaderT<Number = NumberFor<Block>>, // your block's header
{
    type Difficulty = U256;

    fn difficulty(&self, _parent: &H) -> Result<Self::Difficulty, PowError<H>> {
        // For simplicity, return a constant difficulty. Real world PoW would adjust it.
        Ok(U256::from(0x0000_0000_0000_ffffu64))
    }

    fn verify(
        &self,
        header: &H,
        parent: &H,
        difficulty: Self::Difficulty,
    ) -> Result<bool, PowError<H>> {
        // 1) Extract the nonce from the block header's digest
        let mut nonce_bytes = [0u8; 8];
        let mut found_nonce = false;

        for log in header.digest().logs() {
            if let DigestItem::Other(data) = log {
                // We'll parse the first 8 bytes. Use a marker prefix if you like.
                if data.len() >= 8 {
                    nonce_bytes.copy_from_slice(&data[0..8]);
                    found_nonce = true;
                    break;
                }
            }
        }

        if !found_nonce {
            return Ok(false);
        }

        // 2) Build a hash of `(parent_hash, block_number, nonce)`
        let mut hasher = Sha3_256::new();
        hasher.update(parent.hash().as_ref());
        hasher.update(header.number().encode());
        hasher.update(&nonce_bytes);
        let hash_result = U256::from_little_endian(&hasher.finalize());

        // 3) Compare with difficulty
        Ok(hash_result <= difficulty)
    }
}

use std::sync::Arc;
use sc_client_api::HeaderBackend;
use sp_consensus_pow::PowBlockImport;
use sc_service::SpawnTaskHandle;
use sp_api::ProvideRuntimeApi;

/// A simple mining loop that tries nonces until it finds one passing `SimplePowAlgorithm`.
///
/// This is optional. If you skip it, your node won't produce blocks, but can still import them.
pub fn start_mining<B>(
    client: Arc<B>,
    block_import: PowBlockImport<Block, Arc<B>, Arc<B>>,
    spawn_handle: SpawnTaskHandle,
) where
    B: HeaderBackend<Block> + ProvideRuntimeApi<Block> + 'static,
{
    // Spawn a background task for mining
    spawn_handle.spawn("pow-miner", None, async move {
        let mut nonce: u64 = 0;

        loop {
            // 1) Get best block header
            let best_hash = client.info().best_hash;
            let parent_header = client.header(best_hash).unwrap().unwrap();

            // 2) Build a new block using sp_consensus_pow::mine_block
            match sp_consensus_pow::mine_block(
                &block_import,
                &SimplePowAlgorithm,      // Our PoW algo
                parent_header,
                nonce.to_le_bytes().to_vec(), // embed as digest item
            ) {
                Ok(Some((new_header, _))) => {
                    // A valid PoW solution was found; the block was imported/broadcast
                    tracing::info!(
                        target: "pow", 
                        "Mined block #{} with nonce={}",
                        new_header.number(),
                        nonce
                    );
                }
                Ok(None) => {
                    // No valid solution yet; increment nonce
                    nonce = nonce.wrapping_add(1);
                }
                Err(e) => {
                    tracing::error!(target: "pow", "Error while mining: {:?}", e);
                }
            }

            // Throttle so we don't burn 100% CPU in this example
            // Real miners might do multi-threaded hashing, GPU, etc.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
}