//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

use futures::FutureExt;
use sc_client_api::{Backend, BlockBackend};
use sc_service::{error::Error as ServiceError, Configuration, TaskManager, WarpSyncConfig};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::OffchainTransactionPoolFactory;
use sha3pow::{Compute, MinimalSha3Algorithm};
use solochain_template_runtime::{self, apis::RuntimeApi, opaque::Block};
use sp_core::{H256, U256};
use sp_inherents::CreateInherentDataProviders;

use std::{sync::Arc, time::Duration};

use jsonrpsee::tokio;

pub(crate) type FullClient = sc_service::TFullClient<
    Block,
    RuntimeApi,
    sc_executor::WasmExecutor<sp_io::SubstrateHostFunctions>,
>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;

pub type PowBlockImport = sc_consensus_pow::PowBlockImport<
    Block,
    Arc<FullClient>,
    FullClient,
    FullSelectChain,
    MinimalSha3Algorithm,
    impl sp_inherents::CreateInherentDataProviders<Block, ()>,
>;
pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::FullPool<Block, FullClient>,
    (PowBlockImport, Option<Telemetry>),
>;

pub fn build_inherent_data_providers(
) -> Result<impl sp_inherents::CreateInherentDataProviders<Block, ()>, ServiceError> {
    Ok(|_parent, _extra: ()| async move {
        let provider = sp_timestamp::InherentDataProvider::from_system_time();
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(provider)
    })
}

pub fn new_partial(config: &Configuration) -> Result<Service, ServiceError> {
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

    let executor = sc_service::new_wasm_executor::<sp_io::SubstrateHostFunctions>(&config.executor);
    let (client, backend, keystore_container, task_manager) =
        sc_service::new_full_parts::<Block, RuntimeApi, _>(
            config,
            telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
            executor,
        )?;
    let client = Arc::new(client);

    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });

    let select_chain = sc_consensus::LongestChain::new(backend.clone());

    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        config.role.is_authority().into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    let inherent_data_providers = build_inherent_data_providers()?;
    let pow_block_import = sc_consensus_pow::PowBlockImport::new(
        client.clone(),
        client.clone(),
        sha3pow::MinimalSha3Algorithm,
        0, // check inherents starting at block 0
        select_chain.clone(),
        inherent_data_providers,
    );

    let import_queue = sc_consensus_pow::import_queue(
        Box::new(pow_block_import.clone()),
        None,
        sha3pow::MinimalSha3Algorithm,
        &task_manager.spawn_essential_handle(),
        config.prometheus_registry(),
    )?;

    Ok(sc_service::PartialComponents {
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

/// Builds a new service for a full client.
pub fn new_full<
    N: sc_network::NetworkBackend<Block, <Block as sp_runtime::traits::Block>::Hash>,
>(
    config: Configuration,
) -> Result<TaskManager, ServiceError> {
    let sc_service::PartialComponents {
        client,
        backend,
        mut task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (pow_block_import, mut telemetry),
    } = new_partial(&config)?;

    let net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as sp_runtime::traits::Block>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());
    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) =
        sc_service::build_network(sc_service::BuildNetworkParams {
            config: &config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            block_announce_validator_builder: None,
            warp_sync_config: None,
            block_relay: None,
            metrics,
        })?;

    if config.offchain_worker.enabled {
        task_manager.spawn_handle().spawn(
            "offchain-workers-runner",
            "offchain-worker",
            sc_offchain::OffchainWorkers::new(sc_offchain::OffchainWorkerOptions {
                runtime_api_provider: client.clone(),
                is_validator: config.role.is_authority(),
                keystore: Some(keystore_container.keystore()),
                offchain_db: backend.offchain_storage(),
                transaction_pool: Some(OffchainTransactionPoolFactory::new(
                    transaction_pool.clone(),
                )),
                network_provider: Arc::new(network.clone()),
                enable_http_requests: true,
                custom_extensions: |_| vec![],
            })
            .run(client.clone(), task_manager.spawn_handle())
            .boxed(),
        );
    }

    let role = config.role;
    let prometheus_registry = config.prometheus_registry().cloned();

    let rpc_extensions_builder = {
        let client = client.clone();
        let pool = transaction_pool.clone();

        Box::new(move |_| {
            let deps = crate::rpc::FullDeps {
                client: client.clone(),
                pool: pool.clone(),
            };
            crate::rpc::create_full(deps).map_err(Into::into)
        })
    };

    let _rpc_handlers = sc_service::spawn_tasks(sc_service::SpawnTasksParams {
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

    if role.is_authority() {

        let proposer = sc_basic_authorship::ProposerFactory::new(
            task_manager.spawn_handle(),
            client.clone(),
            transaction_pool,
            prometheus_registry.as_ref(),
            None, // lets worry about telemetry later! TODO
        );

        // let can_author_with =
        // 	sp_consensus::CanAuthorWithNativeVersion::new(client.executor().clone());

        let inherent_data_providers = build_inherent_data_providers()?;

        // Parameter details:
        //   https://substrate.dev/rustdocs/v3.0.0/sc_consensus_pow/fn.start_mining_worker.html
        // Also refer to kulupu config:
        //   https://github.com/kulupu/kulupu/blob/master/src/service.rs

        let (worker_handle, worker_task) = sc_consensus_pow::start_mining_worker(
            //block_import: BoxBlockImport<Block>,
            Box::new(pow_block_import),
            client,
            select_chain,
            MinimalSha3Algorithm,
            proposer, // Env E == proposer! TODO
            /*sync_oracle:*/ sync_service.clone(),
            /*justification_sync_link:*/ sync_service.clone(),
            //pre_runtime: Option<Vec<u8>>,
            None,
            inherent_data_providers,
            // time to wait for a new block before starting to mine a new one
            Duration::from_secs(10),
            // how long to take to actually build the block (i.e. executing extrinsics)
            Duration::from_secs(10),
        );

        task_manager
            .spawn_essential_handle()
            .spawn_blocking("pow", None, worker_task);

        task_manager.spawn_essential_handle().spawn(
            "pow-mining-actualy-real-mining-happening",
            None,
            async move {
                let mut nonce = 0;
                loop {
                    // Get mining metadata
                    println!("getting metadata");

                    let metadata = match worker_handle.metadata() {
                        Some(m) => m,
                        None => {
                            log::warn!(target: "pow", "No mining metadata available");
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                            continue;
                        }
                    };
                    let version = worker_handle.version();

                    println!("mine block");

                    // Mine the block
                    let seal =
					match mine_block::<Block>(metadata.pre_hash, H256::from_low_u64_be(nonce), metadata.difficulty) {
                            Ok(s) => {
                                println!("valid seal: {:?}", s);
                                s
                            }
                            Err(_) => {
                                println!("error - seal not valid");
                                nonce += 1;
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                        };

                    println!("block found");

                    let current_version = worker_handle.version();
                    if current_version == version {
                        if futures::executor::block_on(worker_handle.submit(seal.encode())) {
                            println!("Successfully mined and submitted a new block");
                            nonce = 0;
                        } else {
                            println!("Failed to submit mined block");
                            nonce += 1;
                        }
                    }

                    // Sleep to avoid spamming
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                }
            }, // .boxed()
        );

        println!("⛏️  Pow miner spawned");
    }

    network_starter.start_network();
    Ok(task_manager)
}

use codec::Encode;
use sp_runtime::traits::Block as BlockT;
use sha3pow::hash_meets_difficulty;
use sc_consensus_pow::PowAlgorithm;
use sp_runtime::generic::BlockId;

fn mine_block<B: BlockT<Hash = H256>>(
    pre_hash: B::Hash,
    nonce: H256,
    difficulty: U256,
) -> Result<sha3pow::Seal, ()> {
    // Create Compute object
    let compute = Compute {
        difficulty,
        pre_hash: H256::from_slice(pre_hash.as_ref()),
        nonce,
    };

    // Compute the seal
    let seal = compute.compute();

    // // Verify the seal meets difficulty
    if !hash_meets_difficulty(&seal.work, difficulty) {
        return Err(());
    }

    // Verify using the PoW algorithm
    let pow_algorithm = MinimalSha3Algorithm;
	let block_id: BlockId<B> = sp_runtime::generic::BlockId::<B>::hash(pre_hash);
	let is_valid = pow_algorithm
    	.verify(&block_id, 
			&pre_hash, None, &seal.encode(), difficulty)
   	 	.map_err(|_| ())?;
	// let is_valid = pow_algorithm
    //     .verify(&block_id, &H256::from_slice(pre_hash.as_ref()), None, &seal.encode(), difficulty)
    //     .map_err(|_| ())?;

    if is_valid {
        Ok(seal)
    } else {
        Err(())
    }
}