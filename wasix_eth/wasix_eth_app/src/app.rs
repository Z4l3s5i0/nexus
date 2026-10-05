use crate::cli::Args;
use crate::jwt::HeaderInjectorLayer;
use crate::jwt::JwtAuthLayer;
use crate::node::Node;
use jsonrpsee::server::middleware::rpc::RpcServiceBuilder;
use jsonrpsee::server::Server;
use prometheus::{Encoder, TextEncoder};
use serde::{Deserialize, Serialize};
use serde_json::from_reader;
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use wasix_eth_rpc::AdminService;
use wasix_eth_rpc::DebugService;
use wasix_eth_rpc::EngineService;
use wasix_eth_rpc::EthService;
use wasix_eth_rpc::NetService;
use wasix_eth_rpc::Web3Service;
use wasix_eth_rpc::RpcServerFacade;
use wasix_eth_types::genesis::GenesisConfiguration;
use wasix_eth_utils::logging;
use wasix_eth_utils::logging::LogLevel;
use wasix_eth_utils::{debug, error, info, exp};

#[derive(Serialize, Deserialize, Debug)]
struct RegisterNodeRequest {
    pub id: String,
    pub network: String,
    pub chain_id: Option<u64>,
    pub client: String,
    pub rpc_url: String,
    pub metrics_url: Option<String>,
    pub p2p_addr: Option<String>,
    pub discovery_addr: Option<String>,
    pub enode: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BootstrapNode {
    pub id: String,
    pub network: String,
    pub chain_id: Option<u64>,
    pub client: String,
    pub enode: Option<String>,
    pub p2p_addr: Option<String>,
    pub discovery_addr: Option<String>,
    pub last_seen_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BootstrapNodesResponse {
    pub nodes: Vec<BootstrapNode>,
}

pub struct App {
    args: Args,
    node: Option<Node>,
    eth_rpc_server: Option<Server>,
    auth_rpc_server: Option<Server<tower::layer::util::Stack<HeaderInjectorLayer, tower::layer::util::Identity>, tower::layer::util::Stack<JwtAuthLayer, tower::layer::util::Identity>>>,
    eth_module: Option<jsonrpsee::RpcModule<()>>,
    auth_module: Option<jsonrpsee::RpcModule<()>>,
    metrics_handle: Option<tokio::task::JoinHandle<()>>,
}

impl App {
    pub fn builder() -> AppBuilder {
        AppBuilder::default()
    }
    pub fn node(&self) -> Option<&Node> {
        self.node.as_ref()
    }

    pub async fn run(mut self) -> Result<(), Box<dyn Error>> {
        info!("Setup complete...");

        let common_args = match &self.args.command {
            Some(crate::cli::Commands::Run { common }) => Some(common),
            Some(crate::cli::Commands::Init { common }) => Some(common),
            Some(crate::cli::Commands::Import { common }) => Some(common),
            None => None,
        }.ok_or("Common args not found")?;

        if let Some(registry_url) = &common_args.bootstrap_registry {
            info!("Bootstrap registry provided: {}", registry_url);
            let registry_url = registry_url.clone();
            let common_args = common_args.clone();
            let node = self.node.as_ref().unwrap().clone();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_bootstrap_registry_task(registry_url, common_args, node).await {
                    error!("Failed to handle bootstrap registry: {}", e);
                }
            });
        }

        info!("Starting servers...");


        tokio::spawn(async move {
            loop {
                debug!("[App] Runtime heartbeat");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        });

        let eth_module = self.eth_module.take().ok_or("Eth module not initialized")?;
        let auth_module = self.auth_module.take().ok_or("Auth module not initialized")?;
        let eth_rpc_server = self.eth_rpc_server.take().ok_or("Eth RPC server not initialized")?;
        let auth_rpc_server = self.auth_rpc_server.take().ok_or("Auth RPC server not initialized")?;

        let eth_rpc_handle = eth_rpc_server.start(eth_module);
        let auth_rpc_handle = auth_rpc_server.start(auth_module);

        let metrics_port = common_args.metrics_port;
        // info!("Metrics server DISABLED (hyper 1.0 migration pending) on port {}...", metrics_port);

        let addr = SocketAddr::from(([0, 0, 0, 0], metrics_port));
        // info!("Starting metrics server on {}...", addr);
        // let make_svc = hyper::service::service_fn(|_conn| async {
        //     Ok::<_, hyper::Error>(service_fn(|req: Request<Body>| async move {
        //         if req.uri().path() == "/metrics" {
        //             let encoder = TextEncoder::new();
        //             let metric_families = prometheus::gather();
        //             let mut buffer = vec![];
        //             encoder.encode(&metric_families, &mut buffer).unwrap();
        //
        //             let response = Response::builder()
        //                 .status(200)
        //                 .header(hyper::header::CONTENT_TYPE, encoder.format_type())
        //                 .body(Body::from(buffer))
        //                 .unwrap();
        //
        //             Ok::<_, hyper::Error>(response)
        //         } else {
        //             let response = Response::builder()
        //                 .status(404)
        //                 .body(Body::from("Not Found"))
        //                 .unwrap();
        //             Ok::<_, hyper::Error>(response)
        //         }
        //     }))
        // });
        //
        // let server = Server::bind(&addr).serve(make_svc);
        // tokio::spawn(async move {
        //     if let Err(e) = server.await {
        //         error!("Metrics server error: {}", e);
        //     }
        // });
        let node = self.node.as_mut().ok_or("Node not initialized")?;
        node.start(&self.args).await;

        info!("Servers are running, waiting for shutdown signal...");

        let eth_rpc_stop = eth_rpc_handle.stopped();
        let auth_rpc_stop = auth_rpc_handle.stopped();

        tokio::pin!(eth_rpc_stop);
        tokio::pin!(auth_rpc_stop);

        tokio::select! {
            _ = &mut eth_rpc_stop => {
                error!("Eth RPC server stopped internally");
            }
            _ = &mut auth_rpc_stop => {
                error!("Auth RPC server stopped internally");
            }
            _ = futures_util::future::pending::<()>() => {
                info!("Running forever (ctrl_c not available on this platform)...");
            }
        }
        error!("[App] App::run exiting");

        Ok(())
    }

    async fn handle_bootstrap_registry_task(registry_url: String, common_args: crate::cli::CommonArgs, node: Node) -> Result<(), Box<dyn Error>> {
        let network_payload = node.network_payload.as_ref().ok_or("Network payload not initialized")?;

        let node_id = format!("{:?}", network_payload.discovery_v4.identity().public_key_b512());
        let node_id = node_id.trim_start_matches("B512(").trim_end_matches(')');
        // Ensure we remove any 0x prefix from the hex string for enode and id
        let node_id_raw = node_id.trim_start_matches("0x");

        let enode = format!("enode://{}@{}:{}?discport={}", node_id_raw,
            common_args.ext_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "127.0.0.1".to_string()),
            common_args.p2p_port,
            common_args.discovery_port);

        let p2p_addr = format!("{}:{}",
            common_args.ext_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "0.0.0.0".to_string()),
            common_args.p2p_port);

        let discovery_addr = format!("{}:{}",
            common_args.ext_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "0.0.0.0".to_string()),
            common_args.discovery_port);

        let rpc_url = format!("http://{}:{}",
            common_args.ext_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "127.0.0.1".to_string()),
            common_args.eth_rpc_port);

        use wasix_eth_storage::read_traits::ChainProvider;
        let chain_id = node.read_provider.chain_id().unwrap_or(1);

        let register_req = RegisterNodeRequest {
            id: node_id_raw.to_string(),
            network: format!("{}", chain_id),
            chain_id: Some(chain_id),
            client: "wasix-eth".to_string(),
            rpc_url,
            metrics_url: Some(format!("http://{}:{}",
                common_args.ext_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "127.0.0.1".to_string()),
                common_args.metrics_port)),
            p2p_addr: Some(p2p_addr),
            discovery_addr: Some(discovery_addr),
            enode: Some(enode),
        };

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        // 1. Register self
        if common_args.ext_ip.is_none() {
            panic!("Bootstrap registry provided but no external IP (ext_ip) specified. External IP is required for node registration when using a bootstrap registry.");
        }
        info!("[Bootstrap] Registering node with registry... payload: {:?}", register_req);
        let reg_url = format!("{}/api/nodes/register", registry_url.trim_end_matches('/'));
        match tokio::time::timeout(std::time::Duration::from_secs(12), client.post(&reg_url).json(&register_req).send()).await {
            Ok(Ok(resp)) => {
                if resp.status().is_success() {
                    info!("[Bootstrap] Node registered successfully");
                } else {
                    error!("[Bootstrap] Node registration failed: {}", resp.status());
                }
            }
            Ok(Err(e)) => error!("[Bootstrap] Error sending registration request: {}", e),
            Err(_) => error!("[Bootstrap] Registration request timed out"),
        }

        // 2. Fetch bootstrap nodes
        info!("[Bootstrap] Fetching bootstrap nodes from registry...");
        let fetch_url = format!("{}/api/bootstrap/nodes?network={}&chain_id={}&exclude_id={}",
            registry_url.trim_end_matches('/'),
            register_req.network,
            chain_id,
            node_id);

        match tokio::time::timeout(std::time::Duration::from_secs(12), client.get(&fetch_url).send()).await {
            Ok(Ok(resp)) => {
                if resp.status().is_success() {
                    let bootstrap_nodes: BootstrapNodesResponse = match resp.json().await {
                        Ok(n) => n,
                        Err(e) => {
                            error!("[Bootstrap] Failed to deserialize bootstrap nodes: {}", e);
                            return Err(e.into());
                        }
                    };
                    info!("[Bootstrap] Received {} bootstrap nodes", bootstrap_nodes.nodes.len());
                    for node_info in bootstrap_nodes.nodes {
                        if node_info.id == node_id {
                            debug!("[Bootstrap] Skipping self in bootstrap list");
                            continue;
                        }
                        if let Some(enode_str) = node_info.enode {
                            info!("[Bootstrap] Adding bootstrap node: {}", enode_str);
                            network_payload.peer_manager.dial_enode(&enode_str);
                        }
                    }
                } else {
                    error!("[Bootstrap] Failed to fetch bootstrap nodes: {}", resp.status());
                }
            }
            Ok(Err(e)) => error!("[Bootstrap] Error fetching bootstrap nodes: {}", e),
            Err(_) => error!("[Bootstrap] Fetch bootstrap nodes timed out"),
        }

        Ok(())
    }

    pub async fn init(self) -> Result<(), Box<dyn Error>> {
        info!("[App] Genesis initialized. Exiting.");
        Ok(())
    }

    pub async fn import(self) -> Result<(), Box<dyn Error>> {
        info!("[App] Block import completed. Exiting.");
        Ok(())
    }
}

#[derive(Default)]
pub struct AppBuilder {
    args: Option<Args>,
}

impl AppBuilder {
    pub fn with_config(mut self, args: Args) -> Self {
        self.args = Some(args);
        self
    }
    pub fn setup_genesis(&self, genesis_path: PathBuf) -> Result<GenesisConfiguration, Box<dyn Error>> {
        let genesis_file = std::fs::File::open(genesis_path)?;
        let genesis_config: GenesisConfiguration = from_reader(genesis_file)?;
        Ok(genesis_config)
    }
    async fn setup_logging(&self, args: &Args) {
        let level = match args.common.verbose {
            0 => LogLevel::None,
            1 => LogLevel::Exp,
            2 => LogLevel::Info,
            _ => LogLevel::Debug,
        };
        logging::set_log_level(level);
        wasix_eth_utils::metrics::init_metrics();
    }

    pub fn setup_jwt_secret(&self, args: &Args, data_dir: &PathBuf, storage_name_from_id: &str) -> Result<Option<[u8; 32]>, Box<dyn std::error::Error>> {
        if let Some(ref path) = args.common.auth_rpc_jwt_path {
            info!("[App] Loading JWT secret from {:?}", path);
            let secret_str = std::fs::read_to_string(path)?;
            let mut secret_str = secret_str.trim();
            if secret_str.starts_with("0x") {
                secret_str = &secret_str[2..];
            }
            let mut secret = [0u8; 32];
            hex::decode_to_slice(secret_str, &mut secret)
                .map_err(|e| format!("Invalid JWT secret at {:?}: {}", path, e))?;
            Ok(Some(secret))
        } else {
            let jwt_path = data_dir.join(format!("jwt_{}.hex", storage_name_from_id));
            if jwt_path.exists() {
                info!("[App] Loading existing JWT secret from {:?}", jwt_path);
                let secret_str = std::fs::read_to_string(&jwt_path)?;
                let mut secret_str = secret_str.trim();
                if secret_str.starts_with("0x") {
                    secret_str = &secret_str[2..];
                }
                let mut secret = [0u8; 32];
                hex::decode_to_slice(secret_str, &mut secret)
                    .map_err(|e| format!("Invalid JWT secret at {:?}: {}", jwt_path, e))?;
                Ok(Some(secret))
            } else {
                info!("[App] Generating new JWT secret for this node...");
                let mut secret_bytes = [0u8; 32];
                {
                    use rand::RngCore;
                    rand::thread_rng().fill_bytes(&mut secret_bytes);
                }
                // getrandom::getrandom(&mut secret_bytes)
                //    .map_err(|e| format!("Failed to generate JWT secret: {}", e))?;
                let secret_hex = hex::encode(secret_bytes);
                std::fs::write(&jwt_path, &secret_hex)?;
                info!("[App] New JWT secret saved to {:?}", jwt_path);
                Ok(Some(secret_bytes))
            }
        }
    }

    async fn setup_rpc_services(
        &self,
        node: &Node,
        ext_ip: Option<String>,
    ) -> Result<(jsonrpsee::RpcModule<()>, jsonrpsee::RpcModule<()>), Box<dyn Error>> {
        info!("[App] Setting up RPC services");
        let mut eth_facade = RpcServerFacade::new();
        let mut auth_facade = RpcServerFacade::new();

        let eth_service = EthService {
            engine: node.rpc_engine.clone(),
        };
        let debug_service = DebugService::new(node.mempool.clone(), node.rpc_engine.clone());
        let engine_service = EngineService {
            engine: node.engine.clone(),
        };
        let peer_manager = node.peer_manager.clone().expect("PeerManager is required for RPC");
        let admin_service = AdminService::new(peer_manager.clone(), node.read_provider.clone(), ext_ip);
        let net_service = NetService::new(peer_manager);
        let web3_service = Web3Service::new();

        eth_facade.register_debug(debug_service)?;
        eth_facade.register_eth(eth_service.clone())?;
        eth_facade.register_admin(admin_service)?;
        eth_facade.register_net(net_service)?;
        eth_facade.register_web3(web3_service)?;

        auth_facade.register_engine(engine_service)?;
        auth_facade.register_eth(eth_service)?;
        info!("[App] RPC services set up");
        Ok((eth_facade.into_module(), auth_facade.into_module()))
    }

    async fn setup_core_server(
        &self,
        eth_server_address: SocketAddr,
        auth_server_address: SocketAddr,
        auth_secret: Option<[u8; 32]>,
    ) -> Result<(Server, Server<tower::layer::util::Stack<HeaderInjectorLayer, tower::layer::util::Identity>, tower::layer::util::Stack<JwtAuthLayer, tower::layer::util::Identity>>), Box<dyn Error>> {
        info!("[App] Setting up RPC servers");

        let eth_server = Server::builder()
            .build(eth_server_address)
            .await?;

        let rpc_middleware = RpcServiceBuilder::new();
        let http_middleware = tower::ServiceBuilder::new()
            .layer(HeaderInjectorLayer);
        if let Some(secret) = auth_secret {
            let auth_server = Server::builder()
                .set_rpc_middleware(rpc_middleware.layer(JwtAuthLayer::new(secret)))
                .set_http_middleware(http_middleware)
                .build(auth_server_address)
                .await?;

            info!("[App] RPC servers started");
            Ok((eth_server, auth_server))
        } else {
            error!("[App] JWT authentication key missing: Auth RPC requires JWT authentication");
            Err("JWT authentication key missing for Auth RPC".into())
        }
    }
    pub fn setup_addresses(&self, args: &Args) -> (SocketAddr, SocketAddr) {
        info!("[App] Setting up RPC servers addresses");
        let bind_ip = "0.0.0.0".parse::<std::net::IpAddr>().unwrap();
        let eth_rpc_addr = SocketAddr::new(bind_ip, args.common.eth_rpc_port);
        let auth_rpc_addr = SocketAddr::new(bind_ip, args.common.auth_rpc_port);
        info!("[App] Addresses: Eth RPC: {}, Auth RPC: {}", eth_rpc_addr, auth_rpc_addr);
        (eth_rpc_addr, auth_rpc_addr)
    }

    pub async fn build_init(self) -> Result<App, Box<dyn Error>> {
        debug!("[App] Building app for init");
        let args = self.args.clone().ok_or("Config not provided")?;
        self.setup_logging(&args).await;

        let data_dir = args.common.data_dir.clone().unwrap_or_else(|| PathBuf::from("data"));
        std::fs::create_dir_all(&data_dir)?;

        let genesis_path = args.common.genesis_path.as_ref().ok_or("Genesis path not provided. 'init' command requires --genesis-path")?;
        let genesis_configuration = self.setup_genesis(genesis_path.clone())?;

        let storage_name = args.common.peer_name.clone().unwrap_or_else(|| "default".to_string());
        let _jwt_secret = self.setup_jwt_secret(&args, &data_dir, &storage_name)?
            .unwrap_or([0u8; 32]);

        // Initialize only storage
        Node::new_storage_only(&args, genesis_configuration).await?;

        Ok(App {
            args,
            node: None,
            eth_rpc_server: None,
            auth_rpc_server: None,
            eth_module: None,
            auth_module: None,
            metrics_handle: None,
        })
    }

    pub async fn build_import(self) -> Result<App, Box<dyn Error>> {
        debug!("[App] Building app for import");
        let args = self.args.clone().ok_or("Config not provided")?;
        self.setup_logging(&args).await;

        let data_dir = args.common.data_dir.clone().unwrap_or_else(|| PathBuf::from("data"));
        std::fs::create_dir_all(&data_dir)?;

        let storage_name = args.common.peer_name.clone().unwrap_or_else(|| "default".to_string());
        let _jwt_secret = self.setup_jwt_secret(&args, &data_dir, &storage_name)?
            .unwrap_or([0u8; 32]);

        // Initialize Node without genesis - this will fail if genesis hasn't been initialized
        let node = Node::new_no_init(&args).await
            .map_err(|e| format!("Import failed: Genesis must be initialized before importing blocks. Error: {}", e))?;

        // Hive block import
        node.import_blocks(args.common.import_chain.clone(), args.common.import_blocks.clone()).await?;

        Ok(App {
            args,
            node: Some(node),
            eth_rpc_server: None,
            auth_rpc_server: None,
            eth_module: None,
            auth_module: None,
            metrics_handle: None,
        })
    }

    pub async fn build(self) -> Result<App, Box<dyn Error>> {
        debug!("[App] Building app");
        let args = self.args.clone().ok_or("Config not provided")?;
        self.setup_logging(&args).await;

        let data_dir = args.common.data_dir.clone().unwrap_or_else(|| PathBuf::from("data"));
        std::fs::create_dir_all(&data_dir)?;
        
        let genesis_configuration = if let Some(genesis_path) = &args.common.genesis_path {
            Some(self.setup_genesis(genesis_path.clone())?)
        } else {
            None
        };

        let storage_name = args.common.peer_name.clone().unwrap_or_else(|| "default".to_string());
        let jwt_secret = self.setup_jwt_secret(&args, &data_dir, &storage_name)?
            .unwrap_or([0u8; 32]);

        // Initialize Node (Coordinator)
        let node = Node::new(&args, genesis_configuration).await?;

        // Setup RPC services
        let (eth_rpc_addr, auth_rpc_addr) = self.setup_addresses(&args);

        let ext_ip_str = args.common.ext_ip.map(|ip| ip.to_string());
        let (eth_module, auth_module) = self.setup_rpc_services(&node, ext_ip_str).await?;
        let (eth_rpc_server, auth_rpc_server) = self.setup_core_server(eth_rpc_addr, auth_rpc_addr, Some(jwt_secret)).await?;

        debug!("[App] App built with all services");
        Ok(App {
            args,
            node: Some(node),
            eth_rpc_server: Some(eth_rpc_server),
            auth_rpc_server: Some(auth_rpc_server),
            eth_module: Some(eth_module),
            auth_module: Some(auth_module),
            metrics_handle: None,
        })
    }
}
