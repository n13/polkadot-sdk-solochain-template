// inner_import.rs

use std::sync::Arc;
use sc_consensus::{BlockImport, BlockCheckParams, BlockImportParams, ImportResult};
use sp_consensus::Error as ConsensusError;

// Import your concrete block type.
use resonance_runtime_1::opaque::Block; 
// Also import your FullClient and FullBackend from your service module.
// (Adjust the path as needed.)
use crate::service::FullClient;

/// A simple adapter that wraps the client as a block importer.
pub struct ClientBlockImport(pub Arc<FullClient>);

#[jsonrpsee::core::async_trait]
impl BlockImport<Block> for ClientBlockImport {
    type Error = ConsensusError;

    async fn check_block(
        &self,
        block: BlockCheckParams<Block>,
    ) -> Result<ImportResult, Self::Error> {
        // Delegate to the client's check_block method.
        self.0.check_block(block).await.map_err(|e| e.into())
    }

    async fn import_block(
        &self,
        block: BlockImportParams<Block>,
    ) -> Result<ImportResult, Self::Error> {
        // Delegate to the client's import_block method.
        self.0.import_block(block).await.map_err(|e| e.into())
    }
}

/// A helper function to create a new inner block import.
/// This simply wraps the client inside our ClientBlockImport.
pub fn new_inner_block_import(
    _config: &sc_service::Configuration, 
    client: Arc<FullClient>,
) -> Result<ClientBlockImport, sc_service::error::Error> {
    Ok(ClientBlockImport(client))
}