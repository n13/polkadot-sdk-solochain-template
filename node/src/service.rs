//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

use futures::FutureExt;
use resonance_runtime_1::{self, apis::RuntimeApi, opaque::Block};
use sc_client_api::{Backend, BlockBackend};
use sc_consensus::BlockImport;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager, WarpSyncConfig};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::OffchainTransactionPoolFactory;
use std::sync::Arc;
use sc_consensus_pow::import_queue;

// PoW imports // TODO
use sc_consensus_pow::{PowAlgorithm, PowBlockImport, PowVerifier};
use sp_consensus_pow::DifficultyApi;
use crate::pow; 
use crate::inner_import::new_inner_block_import;

pub(crate) type FullClient = sc_service::TFullClient<
    Block,
    RuntimeApi,
    sc_executor::WasmExecutor<sp_io::SubstrateHostFunctions>,
>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;

pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus_pow::PowImportQueue<Block>, /* ImportQueue type – now using PoW import queue */
    sc_transaction_pool::FullPool<Block, FullClient>,
    (Option<Telemetry>,),
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
        sc_service::new_full_parts::<Block, RuntimeApi, _>(
            config,
            telemetry.as_ref().map(|(_, t)| t.handle()),
            executor,
        )?;
    let client = Arc::new(client);

    // Spawn telemetry worker if enabled.
    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });

    // ── Chain Selector and Transaction Pool ───────────────────────────
    let select_chain = sc_consensus::LongestChain::new(backend.clone());
    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        config.role.is_authority().into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    // ── PoW Algorithm Instance ────────────────────────────────────────
    // Create your concrete PoW algorithm instance.
    let pow_algorithm = pow::PowAlgorithmImpl;

    // ── Inherent Data Providers ───────────────────────────────────────
    // Define a closure that creates the inherent data providers.
    // (Adjust this to match what your runtime expects; typically, a timestamp and any other inherent.)
	let create_inherent_data_providers = move |_parent_hash, _| async move {
		let timestamp = sp_timestamp::InherentDataProvider::from_system_time();
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

    // ── PoW Block Import ───────────────────────────────────────────────
    // Wrap the inner block import in a PowBlockImport.
    let pow_block_import = PowBlockImport::new(
        inner_block_import, // inner block import (implements BlockImport<Block>)
        client.clone(),
        pow_algorithm.clone(),
        check_inherents_after, // threshold for checking inherents
        select_chain.clone(),
        create_inherent_data_providers, // inherent data providers closure
    );

    // Box the PowBlockImport so that it can be passed to the import queue.
	let boxed_pow_block_import: Box<
    	dyn BlockImport<Block, Error = sp_consensus::Error> + Send + Sync
	> = Box::new(pow_block_import);
    // For PoW we usually do not have a justification import.
    let justification_import = None;

    // ── PoW Import Queue ───────────────────────────────────────────────
    let import_queue = import_queue(
        boxed_pow_block_import,
        justification_import,
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
        other: (telemetry,),
    })
    // unimplemented!("not done ");

    // let cidp_client = client.clone();

    // // PoW import queue
    // let pow_verifier = PowVerifier::new(
    //     client.clone(),
    //     // Provide your PoW algorithm (e.g., SHA3 hasher)
    //     PowAlgorithm,
    //     // Difficulty calculator (custom or from runtime)
    //     difficulty_calculator,
    // );
    // let import_queue = sc_consensus_pow::import_queue(
    //     ImportQueueParams {
    //         block_import: pow_block_import.clone(),
    //         justification_import: None, // PoW doesn't use justifications
    //         client: client.clone(),
    //         create_inherent_data_providers: move |_, _| async move {
    //             Ok(()) // Adjust inherent data as needed
    //         },
    //         spawner: &task_manager.spawn_essential_handle(),
    //         registry: config.prometheus_registry(),
    //         check_for_equivocation: Default::default(),
    //         telemetry: telemetry.as_ref().map(|x| x.handle()),
    //     },
    //     pow_verifier,
    // )?;

    // TODO make our own -
    // 	let import_queue =
    // 		sc_consensus_aura::import_queue::<AuraPair, _, _, _, _, _>(ImportQueueParams {
    // 			block_import: grandpa_block_import.clone(),
    // 			justification_import: Some(Box::new(grandpa_block_import.clone())),
    // 			client: client.clone(),
    // 			create_inherent_data_providers: move |parent_hash, _| {
    // 				let cidp_client = cidp_client.clone();
    // 				async move {
    // 					let slot_duration = sc_consensus_aura::standalone::slot_duration_at(
    // 						&*cidp_client,
    // 						parent_hash,
    // 					)?;
    // 					let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

    // 					let slot =
    // 						sp_consensus_aura::inherents::InherentDataProvider::from_timestamp_and_slot_duration(
    // 							*timestamp,
    // 							slot_duration,
    // 						);

    // 					Ok((slot, timestamp))
    // 				}
    // 			},
    // 			spawner: &task_manager.spawn_essential_handle(),
    // 			registry: config.prometheus_registry(),
    // 			check_for_equivocation: Default::default(),
    // 			telemetry: telemetry.as_ref().map(|x| x.handle()),
    // 			compatibility_mode: Default::default(),
    // 		})?;

    // Ok(sc_service::PartialComponents {
    // 	client,
    // 	backend,
    // 	task_manager,
    // 	import_queue,
    // 	keystore_container,
    // 	select_chain,
    // 	transaction_pool,
    // 	other: (
    // 		//grandpa_block_import,
    // 		//grandpa_link,
    // 		telemetry),
    // })
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
		other: (
			//block_import,  // TODO add
			//grandpa_link, 
			mut telemetry),
	} = new_partial(&config)?;

    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as sp_runtime::traits::Block>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());
    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let peer_store_handle = net_config.peer_store_handle();
    // let grandpa_protocol_name = sc_consensus_grandpa::protocol_standard_name(
    // 	&client.block_hash(0).ok().flatten().expect("Genesis block exists; qed"),
    // 	&config.chain_spec,
    // );
    // let (grandpa_protocol_config, grandpa_notification_service) =
    // 	sc_consensus_grandpa::grandpa_peers_set_config::<_, N>(
    // 		grandpa_protocol_name.clone(),
    // 		metrics.clone(),
    // 		peer_store_handle,
    // 	);

    // TODO do
    // net_config.add_notification_protocol(grandpa_protocol_config);

    // let warp_sync = Arc::new(sc_consensus_grandpa::warp_proof::NetworkProvider::new(
    // 	backend.clone(),
    // 	grandpa_link.shared_authority_set().clone(),
    // 	Vec::default(),
    // ));

    // TODO do
    // let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) =
    // 	sc_service::build_network(sc_service::BuildNetworkParams {
    // 		config: &config,
    // 		net_config,
    // 		client: client.clone(),
    // 		transaction_pool: transaction_pool.clone(),
    // 		spawn_handle: task_manager.spawn_handle(),
    // 		import_queue,
    // 		block_announce_validator_builder: None,
    // 		warp_sync_config: Some(WarpSyncConfig::WithProvider(warp_sync)),
    // 		block_relay: None,
    // 		metrics,
    // 	})?;

    if config.offchain_worker.enabled {
        // task_manager.spawn_handle().spawn(
        // 	"offchain-workers-runner",
        // 	"offchain-worker",
        // 	sc_offchain::OffchainWorkers::new(sc_offchain::OffchainWorkerOptions {
        // 		runtime_api_provider: client.clone(),
        // 		is_validator: config.role.is_authority(),
        // 		keystore: Some(keystore_container.keystore()),
        // 		offchain_db: backend.offchain_storage(),
        // 		transaction_pool: Some(OffchainTransactionPoolFactory::new(
        // 			transaction_pool.clone(),
        // 		)),
        // 		network_provider: Arc::new(network.clone()),
        // 		enable_http_requests: true,
        // 		custom_extensions: |_| vec![],
        // 	})
        // 	.run(client.clone(), task_manager.spawn_handle())
        // 	.boxed(),
        // );
    }

    let role = config.role;
    let force_authoring = config.force_authoring;
    let backoff_authoring_blocks: Option<()> = None;
    let name = config.network.node_name.clone();
    // let enable_grandpa = !config.disable_grandpa;
    let prometheus_registry = config.prometheus_registry().cloned();

    // let rpc_extensions_builder = {
    // 	let client = client.clone();
    // 	let pool = transaction_pool.clone();

    // 	Box::new(move |_| {
    // 		let deps = crate::rpc::FullDeps { client: client.clone(), pool: pool.clone() };
    // 		crate::rpc::create_full(deps).map_err(Into::into)
    // 	})
    // };

    // let _rpc_handlers = sc_service::spawn_tasks(sc_service::SpawnTasksParams {
    // 	network: Arc::new(network.clone()),
    // 	client: client.clone(),
    // 	keystore: keystore_container.keystore(),
    // 	task_manager: &mut task_manager,
    // 	transaction_pool: transaction_pool.clone(),
    // 	rpc_builder: rpc_extensions_builder,
    // 	backend,
    // 	system_rpc_tx,
    // 	tx_handler_controller,
    // 	sync_service: sync_service.clone(),
    // 	config,
    // 	telemetry: telemetry.as_mut(),
    // })?;

    // TODO impl block authoring

    // if role.is_authority() {
    // 	let proposer_factory = sc_basic_authorship::ProposerFactory::new(
    // 		task_manager.spawn_handle(),
    // 		client.clone(),
    // 		transaction_pool.clone(),
    // 		prometheus_registry.as_ref(),
    // 		telemetry.as_ref().map(|x| x.handle()),
    // 	);

    // 	let slot_duration = sc_consensus_aura::slot_duration(&*client)?;

    // 	let aura = sc_consensus_aura::start_aura::<AuraPair, _, _, _, _, _, _, _, _, _, _>(
    // 		StartAuraParams {
    // 			slot_duration,
    // 			client,
    // 			select_chain,
    // 			block_import,
    // 			proposer_factory,
    // 			create_inherent_data_providers: move |_, ()| async move {
    // 				let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

    // 				let slot =
    // 					sp_consensus_aura::inherents::InherentDataProvider::from_timestamp_and_slot_duration(
    // 						*timestamp,
    // 						slot_duration,
    // 					);

    // 				Ok((slot, timestamp))
    // 			},
    // 			force_authoring,
    // 			backoff_authoring_blocks,
    // 			keystore: keystore_container.keystore(),
    // 			sync_oracle: sync_service.clone(),
    // 			justification_sync_link: sync_service.clone(),
    // 			block_proposal_slot_portion: SlotProportion::new(2f32 / 3f32),
    // 			max_block_proposal_slot_portion: None,
    // 			telemetry: telemetry.as_ref().map(|x| x.handle()),
    // 			compatibility_mode: Default::default(),
    // 		},
    // 	)?;

    // 	// the AURA authoring task is considered essential, i.e. if it
    // 	// fails we take down the service with it.
    // 	task_manager
    // 		.spawn_essential_handle()
    // 		.spawn_blocking("aura", Some("block-authoring"), aura);
    // }

    // if enable_grandpa {
    // 	// if the node isn't actively participating in consensus then it doesn't
    // 	// need a keystore, regardless of which protocol we use below.
    // 	let keystore = if role.is_authority() { Some(keystore_container.keystore()) } else { None };

    // 	let grandpa_config = sc_consensus_grandpa::Config {
    // 		// FIXME #1578 make this available through chainspec
    // 		gossip_duration: Duration::from_millis(333),
    // 		justification_generation_period: GRANDPA_JUSTIFICATION_PERIOD,
    // 		name: Some(name),
    // 		observer_enabled: false,
    // 		keystore,
    // 		local_role: role,
    // 		telemetry: telemetry.as_ref().map(|x| x.handle()),
    // 		protocol_name: grandpa_protocol_name,
    // 	};

    // 	// start the full GRANDPA voter
    // 	// NOTE: non-authorities could run the GRANDPA observer protocol, but at
    // 	// this point the full voter should provide better guarantees of block
    // 	// and vote data availability than the observer. The observer has not
    // 	// been tested extensively yet and having most nodes in a network run it
    // 	// could lead to finality stalls.
    // 	let grandpa_config = sc_consensus_grandpa::GrandpaParams {
    // 		config: grandpa_config,
    // 		link: grandpa_link,
    // 		network,
    // 		sync: Arc::new(sync_service),
    // 		notification_service: grandpa_notification_service,
    // 		voting_rule: sc_consensus_grandpa::VotingRulesBuilder::default().build(),
    // 		prometheus_registry,
    // 		shared_voter_state: SharedVoterState::empty(),
    // 		telemetry: telemetry.as_ref().map(|x| x.handle()),
    // 		offchain_tx_pool_factory: OffchainTransactionPoolFactory::new(transaction_pool),
    // 	};

    // 	// the GRANDPA voter task is considered infallible, i.e.
    // 	// if it fails we take down the service with it.
    // 	task_manager.spawn_essential_handle().spawn_blocking(
    // 		"grandpa-voter",
    // 		None,
    // 		sc_consensus_grandpa::run_grandpa_voter(grandpa_config)?,
    // 	);
    // }

    // network_starter.start_network();
    unimplemented!("not implrmenets");
    Ok(task_manager)
}
