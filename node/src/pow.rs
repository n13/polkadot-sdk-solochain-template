// my_pow_impl.rs

use codec::{Decode, Encode}; use sp_core::blake2_256;
// Import the Encode traituse sp_core::hashing::blake2_256;
use sp_runtime::generic::BlockId;
use sc_consensus_pow::{Error, PowAlgorithm};
use sp_consensus_pow::Seal;
use sp_runtime::traits::Block as BlockT;

/// A PoW algorithm that checks if the computed hash meets the difficulty target.
#[derive(Clone)]
pub struct PowAlgorithmImpl;

impl<B: BlockT> PowAlgorithm<B> for PowAlgorithmImpl {
    type Difficulty = u128;

    fn difficulty(&self, _parent: B::Hash) -> Result<Self::Difficulty, Error<B>> {
        Ok(1)
    }

    /// Verifies the seal by checking:
    /// 1. The seal is correctly decoded into a nonce and result hash.
    /// 2. The computed hash (from pre_hash + nonce) matches the result hash in the seal.
    /// 3. The numeric value of the computed hash meets the difficulty target.
    fn verify(
        &self,
        _parent: &BlockId<B>,
        pre_hash: &B::Hash,
        _pre_digest: Option<&[u8]>,
        seal: &Seal,
        difficulty: Self::Difficulty,
    ) -> Result<bool, Error<B>> {
        // Decode the seal into a nonce and precomputed hash
        let mut seal_data = &seal[..];
        let (nonce, result_hash) = match <(u64, [u8; 32])>::decode(&mut seal_data) {
            Ok(res) => res,
            Err(_) => return Ok(false), // Invalid seal encoding
        };

        // Compute the hash using the pre_hash and nonce
        let input = (pre_hash, nonce).encode();
        let computed_hash = blake2_256(&input);

        // Verify the computed hash matches the seal's result_hash
        if computed_hash != result_hash {
            return Ok(false);
        }

        // Convert the first 16 bytes of the hash into a u128 (little-endian)
        let hash_num = u128::from_le_bytes(
            computed_hash[0..16]
                .try_into()
                .expect("First 16 bytes of hash are always valid"),
        );

        // Check if the hash value meets the difficulty target
        Ok(hash_num <= difficulty)
    }
}