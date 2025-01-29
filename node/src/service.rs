//! Service and ServiceFactory implementation. Specialized wrapper over Substrate service.

use std::{sync::Arc, time::Duration};

use sc_client_api::{Backend, BlockBackend};
use sc_service::{
    build_network, error::Error as ServiceError, spawn_tasks, Configuration, PartialComponents,
    SpawnTasksParams, TaskManager,
};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::OffchainTransactionPoolFactory;
use sc_offchain::OffchainWorkers;
use sc_network::config::FullNetworkConfiguration;
use sp_runtime::traits::Block as BlockT;
use sp_consensus::SyncOracle;
use sp_inherents::CreateInherentDataProviders;
use sc_consensus::{LongestChain, NeverDefault}; // Consensus utilities

use sc_consensus_pow::{start_mining_worker, PowBlockImport, import_queue as pow_import_queue};
use sp_consensus_pow::PowAlgorithm;
use sc_basic_authorship::ProposerFactory;

use solochain_template_runtime::{self, apis::RuntimeApi, opaque::Block};
use crate::{
    rpc,                    // Custom RPC module
    pow::SimplePowAlgorithm, // Your PoW trait implementation
};

/// Full client type.
pub(crate) type FullClient = sc_service::TFullClient<
    Block,
    RuntimeApi,
    sc_executor::WasmExecutor<sp_io::SubstrateHostFunctions>,
>;

type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = LongestChain<FullBackend, Block>;

/// Type alias for service components.
pub type Service = PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::FullPool<Block, FullClient>,
    (PowBlockImport<Block, FullClient, FullSelectChain, SimplePowAlgorithm, NeverDefault>, Option<Telemetry>),
>;

/// Builds the **partial** node service.
pub fn new_partial(config: &Configuration) -> Result<Service, ServiceError> {
    let telemetry = config.telemetry_endpoints
        .clone()
        .filter(|x| !x.is_empty())
        .map(|endpoints| -> Result<_, sc_telemetry::Error> {
            let worker = TelemetryWorker::new(16)?;
            let telemetry = worker.handle().new_telemetry(endpoints);
            Ok((worker, telemetry))
        })
        .transpose()?;

    let executor = sc_service::new_wasm_executor::<sp_io::SubstrateHostFunctions>(&config.executor);

    let (client, backend, keystore_container, task_manager) =
        sc_service::new_full_parts::<Block, RuntimeApi, _>(
            config,
            telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
            executor,
        )?;
    let client = Arc::new(client);

    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager.spawn_handle().spawn("telemetry", None, worker.run());
        telemetry
    });

    let select_chain = LongestChain::new(backend.clone());

    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        config.role.is_authority().into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    // Dummy block import if no pipeline exists
    let inner_import = sc_service::client::new_dummy_block_import::<Block, _>(client.clone());

    // PoW block import setup
    let pow_block_import = PowBlockImport::new(
        inner_import,              // Inner block import
        client.clone(),            // Client
        SimplePowAlgorithm,        // PoW Algorithm
        0,                         // Check inherents from block number 0
        select_chain.clone(),      // Chain selection
        NeverDefault::default(),   // No inherent providers
    );

    let import_queue = pow_import_queue(
        Box::new(pow_block_import.clone()), // Must be BoxBlockImport
        None,                               // No justification import for PoW
        SimplePowAlgorithm,
        &task_manager.spawn_essential_handle(),
        config.prometheus_registry(),
    )?;

    Ok(PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (pow_block_import, telemetry),
    })
}

/// Builds a new full node service (PoW-based, no Aura/Grandpa).
pub fn new_full<N: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>>(
    config: Configuration,
) -> Result<TaskManager, ServiceError> {
    let PartialComponents {
        client,
        backend,
        mut task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (pow_block_import, mut telemetry),
    } = new_partial(&config)?;

    // Network configuration
    let mut net_config = FullNetworkConfiguration::<Block, <Block as BlockT>::Hash, N>::new(
        &config.network,
        config.prometheus_registry().cloned(),
    );

    let metrics = N::register_notification_metrics(config.prometheus_registry());

    // No warp_sync for PoW
    let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) =
        build_network(sc_service::BuildNetworkParams {
            config: &config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            block_announce_validator_builder: None,
            warp_sync_config: None, // No Grandpa warp sync
            block_relay: None,
            metrics,
        })?;

    // Optional: Offchain Workers
    if config.offchain_worker.enabled {
        task_manager.spawn_handle().spawn(
            "offchain-workers-runner",
            "offchain-worker",
            OffchainWorkers::new(sc_offchain::OffchainWorkerOptions {
                runtime_api_provider: client.clone(),
                is_validator: config.role.is_authority(),
                keystore: Some(keystore_container.keystore()),
                offchain_db: backend.offchain_storage(),
                transaction_pool: Some(OffchainTransactionPoolFactory::new(transaction_pool.clone())),
                network_provider: Arc::new(network.clone()),
                enable_http_requests: true,
                custom_extensions: |_| vec![],
            })
            .run(client.clone(), task_manager.spawn_handle())
            .boxed(),
        );
    }

    // RPC setup
    let rpc_extensions_builder = {
        let client = client.clone();
        let pool = transaction_pool.clone();

        Box::new(move |_| {
            let deps = rpc::FullDeps {
                client: client.clone(),
                pool: pool.clone(),
            };
            rpc::create_full(deps).map_err(Into::into)
        })
    };

    // Spawn standard tasks: RPC, telemetry, etc.
    spawn_tasks(SpawnTasksParams {
        network: Arc::new(network.clone()),
        client: client.clone(),
        keystore: keystore_container.keystore(),
        task_manager: &mut task_manager,
        transaction_pool: transaction_pool.clone(),
        rpc_builder: rpc_extensions_builder,
        backend,
        system_rpc_tx,
        tx_handler_controller,
        sync_service: sync_service.clone(),
        config,
        telemetry: telemetry.as_mut(),
    })?;

    // Start PoW mining worker if authority
    if config.role.is_authority() {
        let proposer_factory = ProposerFactory::new(
            task_manager.spawn_handle(),
            client.clone(),
            transaction_pool.clone(),
            config.prometheus_registry().as_ref(),
            telemetry.as_ref().map(|t| t.handle()),
        );

        let create_inherent_data_providers = NeverDefault::default();

        let (mining_handle, mining_future) = start_mining_worker(
            Box::new(pow_block_import.clone()),
            client.clone(),
            select_chain.clone(),
            SimplePowAlgorithm,    // PoW algorithm
            proposer_factory,      // Block proposer
            sync_service.clone(),  // SyncOracle
            (),                    // No justification sync link
            None,                  // No pre-runtime data
            create_inherent_data_providers,
            Duration::from_secs(15), // Timeout
            Duration::from_secs(2),  // Block build time
        );

        task_manager.spawn_handle().spawn("pow-miner", None, mining_future);
    }

    // Start the network and return TaskManager
    network_starter.start_network();
    Ok(task_manager)
}