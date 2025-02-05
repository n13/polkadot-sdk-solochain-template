//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

use futures::FutureExt;
use jsonrpsee::tokio;
use resonance_runtime_1::{self, apis::RuntimeApi, opaque::Block};
use sc_client_api::{Backend, BlockBackend};
use sc_consensus::BlockImport;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager, WarpSyncConfig};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::OffchainTransactionPoolFactory;
use sp_consensus::Error as ConsensusError;
use sp_inherents::CreateInherentDataProviders;
use sp_timestamp::InherentDataProvider;
use std::sync::Arc;
use sc_consensus_pow::import_queue;
use std::time::Duration;
use sc_consensus_pow::{MiningHandle, MiningMetadata, PowAlgorithm};
use std::future::Future;

// PoW imports // TODO
use sc_consensus_pow::{PowBlockImport, PowVerifier};
use sp_consensus_pow::DifficultyApi;
use crate::pow::PowAlgorithmImpl;
use crate::inner_import::{new_inner_block_import, ClientBlockImport};
use sp_runtime::traits::Block as BlockT;

pub(crate) type FullClient = sc_service::TFullClient<
    Block,
    RuntimeApi,
    sc_executor::WasmExecutor<sp_io::SubstrateHostFunctions>,
>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;

// helper trait
pub trait DynPowBlockImport<Block: BlockT>: BlockImport<Block, Error = ConsensusError> + Send + Sync {}
impl<T, Block> DynPowBlockImport<Block> for T
where
    T: BlockImport<Block, Error = ConsensusError> + Send + Sync,
    Block: BlockT,
{}
// pub type BoxPowBlockImport<Block> = Box<dyn DynPowBlockImport<Block>>;

pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus_pow::PowImportQueue<Block>,
    sc_transaction_pool::FullPool<Block, FullClient>,
    (Box<dyn BlockImport<Block, Error = ConsensusError> + Send + Sync>, Option<Telemetry>),
>;

pub fn new_partial(config: &Configuration) -> Result<Service, ServiceError> {
    // ── Telemetry Setup ──────────────────────────────────────────────
    let telemetry = config
        .telemetry_endpoints
        .clone()
        .filter(|x| !x.is_empty())
        .map(|endpoints| -> Result<_, sc_telemetry::Error> {
            let worker = TelemetryWorker::new(16)?;
            let telemetry = worker.handle().new_telemetry(endpoints);
            Ok((worker, telemetry))
        })
        .transpose()?;

    // ── WASM Executor and Full Client Setup ─────────────────────────
    let executor = sc_service::new_wasm_executor::<sp_io::SubstrateHostFunctions>(&config.executor);
    
    let (client, backend, keystore_container, mut task_manager) =
    sc_service::new_full_parts::<
        Block,
        RuntimeApi,
        sc_executor::WasmExecutor<sp_io::SubstrateHostFunctions>,
    >(config, telemetry.as_ref().map(|(_, t)| t.handle()), executor)?;
    let client = Arc::new(client);

    // Spawn telemetry worker if enabled.
    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });
    println!("🛠️  Creating transaction pool...");
    // ── Chain Selector and Transaction Pool ───────────────────────────
    let select_chain = sc_consensus::LongestChain::new(backend.clone());
    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        config.role.is_authority().into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    println!("🔨 Initializing PoW algorithm instance");

    // ── PoW Algorithm Instance ────────────────────────────────────────
    // Create your concrete PoW algorithm instance.
    let pow_algorithm = PowAlgorithmImpl;

    // ── Inherent Data Providers ───────────────────────────────────────
    // Define a closure that creates the inherent data providers.
    // (Adjust this to match what your runtime expects; typically, a timestamp and any other inherent.)
    let create_inherent_data_providers = move |_parent_hash: <Block as BlockT>::Hash, _extra: ()| async move {
        // Create a timestamp provider from the system time.
        let timestamp = sp_timestamp::InherentDataProvider::from_system_time();
        // Return it as a single-element tuple.
        Ok((timestamp,))
    };

    // ── Inner Block Import ─────────────────────────────────────────────
    // Here you must supply an inner block import that performs the actual state transition.
    // In many nodes this was used for Aura; now you must provide one that works without Aura/GRANDPA.
    // For example, if you have a helper function `new_inner_block_import` that returns an object
    // implementing `BlockImport<Block>`, use it here:
    let inner_block_import = new_inner_block_import(config, client.clone())?;
    // (Alternatively, reuse an import from your previous consensus implementation if appropriate.)

    // Define after which block number inherents are checked. Adjust as needed.
    let check_inherents_after = 0u32.into();

    println!("📦 Configuring PoW block import");
    // ── PoW Block Import ───────────────────────────────────────────────
    // Wrap the inner block import in a PowBlockImport.
    let pow_block_import = PowBlockImport::new(
        inner_block_import,
        client.clone(),
        pow_algorithm.clone(),
        check_inherents_after,
        select_chain.clone(),
        create_inherent_data_providers,
    );

    println!("🚚 Building PoW import queue");
    // ── PoW Import Queue ───────────────────────────────────────────────
    let import_queue = import_queue(
		Box::new(pow_block_import.clone()),
        None,
        pow_algorithm.clone(),
        &task_manager.spawn_essential_handle(),
        config.prometheus_registry(),
    )?;

    // ── Return Partial Components ─────────────────────────────────────
    Ok(sc_service::PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (Box::new(pow_block_import.clone()), telemetry),
    })
}

/// Builds a new service for a full client.
/// 
pub fn new_full<
    N: sc_network::NetworkBackend<Block, <Block as sp_runtime::traits::Block>::Hash>,
>(
    config: Configuration,
) -> Result<TaskManager, ServiceError> {
    let sc_service::PartialComponents::<FullClient, _, _, _, _, _> {
		client,
		backend,
		mut task_manager,
		import_queue,
		keystore_container,
		select_chain,
		transaction_pool,
		other: (pow_block_import, telemetry),
	} = new_partial(&config)?;

    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as sp_runtime::traits::Block>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());
    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let peer_store_handle = net_config.peer_store_handle();
    
    let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) = 
    sc_service::build_network(sc_service::BuildNetworkParams {
        config: &config,
        net_config,
        client: client.clone(),
        transaction_pool: transaction_pool.clone() as Arc<sc_transaction_pool::FullPool<Block, FullClient>>,
        spawn_handle: task_manager.spawn_handle(),
        import_queue,
        block_announce_validator_builder: None,
        warp_sync_config: None, // PoW doesn't use warp sync
        block_relay: None,
        metrics,
    })?;

    if config.offchain_worker.enabled {
        // we don't need this
        // task_manager.spawn_handle().spawn(
        //     "offchain-workers-runner",
        //     "offchain-worker",
        //     sc_offchain::OffchainWorkers::new(sc_offchain::OffchainWorkerOptions {
        //         runtime_api_provider: client.clone(),
        //         is_validator: config.role.is_authority(),
        //         keystore: Some(keystore_container.keystore()),
        //         offchain_db: backend.offchain_storage(),
        //         transaction_pool: Some(OffchainTransactionPoolFactory::new(
        //             transaction_pool.clone() as Arc<sc_transaction_pool::FullPool<Block, FullClient>>
        //         )),
        //         network_provider: Arc::new(network.clone()),
        //         enable_http_requests: true,
        //         custom_extensions: |_| vec![],
        //     })
        //     .run(client.clone(), task_manager.spawn_handle())
        //     .boxed(),
        // );
    }

    let role = config.role;
    let force_authoring = config.force_authoring;
    let backoff_authoring_blocks: Option<()> = None;
    let name = config.network.node_name.clone();
    let prometheus_registry = config.prometheus_registry().cloned();

    // Start the mining worker
    println!("⏳ Checking node role for mining: is_authority={}", role.is_authority());

    if role.is_authority() {
        println!("starting mining worker");
        let proposer_factory = sc_basic_authorship::ProposerFactory::new(
            task_manager.spawn_handle(),
            client.clone(),
            transaction_pool.clone(),
            prometheus_registry.as_ref(),
            telemetry.as_ref().map(|x| x.handle()),
        );

        let pow_algorithm = PowAlgorithmImpl;
        
        // Assume `Block` is your concrete blockchain type.
        let create_inherent_data_providers = move |_parent_hash: <Block as BlockT>::Hash, _extra: ()| async move {
            // Create a timestamp provider from the system time.
            let timestamp = InherentDataProvider::from_system_time();
            // Return it as a single-element tuple.
            Ok((timestamp,))
        };
        let (mining_handle, mining_task) = sc_consensus_pow::start_mining_worker(
            /*block_import:*/ Box::new(pow_block_import),
            /*client:*/ client.clone(),
            /*select_chain:*/ select_chain.clone(),
            /*algorithm:*/ pow_algorithm,
            /*env:*/ proposer_factory,
            /*sync_oracle:*/ sync_service.clone(),
            /*justification_sync_link:*/ sync_service.clone(),
            /*pre_runtime:*/ None,
            /*create_inherent_data_providers:*/ create_inherent_data_providers,
            /*timeout:*/ Duration::from_secs(20),
            /*build_time:*/ Duration::from_secs(10),
        );
    
        println!("⛏️  Starting PoW miner worker");

        task_manager.spawn_essential_handle().spawn(
            "pow-mining",
            None,
            mining_task
        );
        println!("⛏️  Start mining...");
        // assert_send(mining_handle);

        task_manager.spawn_essential_handle().spawn(
            "pow-mining",
            None,
            async move {
                let mut nonce = 0;
                loop {
                    // Get mining metadata
                    let metadata = match mining_handle.metadata() {
                        Some(m) => m,
                        None => {
                            log::warn!(target: "pow", "No mining metadata available");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    };

                    // Mine the block
                    let seal = match mine_block::<Block>(
                        metadata.pre_hash,
                        nonce,
                        metadata.difficulty,
                    ) {
                        Ok(s) => s,
                        Err(_) => {
                            nonce += 1;
                            continue;
                        }
                    };
                    let current_metadata = mining_handle.metadata();
                    if current_metadata == Some(metadata) {
                        if futures::executor::block_on(mining_handle.submit(seal)) {
                            log::debug!(target: "pow", "Successfully mined and submitted a new block");
                            nonce = 0;
                        } else {
                            log::warn!(target: "pow", "Failed to submit mined block");
                            nonce += 1;
                        }
                    }

                    // Submit the mined block
                    // if mining_handle.submit(seal).await {
                    //     log::debug!(target: "pow", "Successfully mined and submitted a new block");
                    //     nonce = 0; // Reset nonce after successful submission
                    // } else {
                    //     log::warn!(target: "pow", "Failed to submit mined block");
                    //     nonce += 1;
                    // }

                    // Sleep to avoid spamming
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            .boxed()
        );

    }

    network_starter.start_network();
    Ok(task_manager)
}

use sp_core::blake2_256;
use sp_consensus_pow::Seal;
use codec::Encode;
use sp_runtime::generic::BlockId;

fn mine_block<B: BlockT>(
    pre_hash: B::Hash,
    nonce: u64,
    difficulty: <PowAlgorithmImpl as PowAlgorithm<B>>::Difficulty,
) -> Result<Seal, ()> {
    let pow_algorithm = PowAlgorithmImpl;

    // Create a dummy BlockId for verification
    let block_id = BlockId::<B>::hash(pre_hash);

    // Compute hash (PoW proof)
    let input = (pre_hash, nonce).encode();
    let computed_hash = blake2_256(&input);

    // Create the seal (nonce + hash)
    let seal = (nonce, computed_hash).encode();

    // Verify the seal using the PoW algorithm
    let is_valid = pow_algorithm.verify(
        &block_id,
        &pre_hash,
        None,
        &seal,
        difficulty,
    ).map_err(|_| ())?;

    if is_valid {
        Ok(seal)
    } else {
        Err(())
    }
}

// helper method
fn assert_send<T: Send>(_: T)  {
    println!("It's send");
}
// pub struct CreateInherentDataProviders;

// #[async_trait]
// impl sp_inherents::CreateInherentDataProviders<Block, ()> for CreateInherentDataProviders {
// 	type InherentDataProviders = sp_timestamp::InherentDataProvider;

// 	async fn create_inherent_data_providers(
// 		&self,
// 		_parent: <Block as BlockT>::Hash,
// 		_extra_args: (),
// 	) -> Result<Self::InherentDataProviders, Box<dyn std::error::Error + Send + Sync>> {
// 		Ok(sp_timestamp::InherentDataProvider::from_system_time())
// 	}
// }
