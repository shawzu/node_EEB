use anyhow::{anyhow, Result};
use libp2p::{
    autonat,
    dcutr,
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode, MessageId},
    identify,
    kad::{self, store::MemoryStore},
    mdns,
    multiaddr::Protocol,
    noise,
    ping,
    relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm, Transport,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};
use tokio::{select, time::interval};
use tracing::{debug, error, info, warn};
use futures::StreamExt;

const PROTOCOL_VERSION: &str = "/node-eeb/1.0.0";
const HANDSHAKE_TOPIC: &str = "node-eeb-handshakes";

const BOOTSTRAP_NODES: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa", 
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zp9Kky4f5RmvJw2e6GrmNw9hxKL1MH",
];

#[derive(Debug, Serialize, Deserialize)]
pub struct HandshakeMessage {
    pub node_name: Option<String>,
    pub peer_id: String,
    pub timestamp: u64,
    pub message: String,
}

#[derive(NetworkBehaviour)]
pub struct P2PBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    relay: relay::Behaviour,
    dcutr: dcutr::Behaviour,
    autonat: autonat::Behaviour,
}

pub struct P2PNode {
    swarm: Swarm<P2PBehaviour>,
    node_name: Option<String>,
    handshake_topic: IdentTopic,
}

impl P2PNode {
    pub async fn new(
        name: Option<String>,
        port: Option<u16>,
        enable_dht: bool,
        enable_mdns: bool,
        use_bootstrap: bool,
        relay_mode: bool,
    ) -> Result<Self> {
        // Create a random key pair for this node
        let local_key = libp2p::identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        
        info!("🆔 Local peer ID: {}", local_peer_id);
        
        // Set up transport with noise encryption and yamux multiplexing
        let transport = tcp::tokio::Transport::default()
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(noise::Config::new(&local_key)
                .map_err(|e| anyhow!("Failed to create noise config: {}", e))?)
            .multiplex(yamux::Config::default())
            .boxed();

        // Create gossipsub configuration
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(ValidationMode::Strict)
            .message_id_fn(|message| {
                let mut hasher = DefaultHasher::new();
                message.data.hash(&mut hasher);
                MessageId::from(hasher.finish().to_string())
            })
            .build()
            .map_err(|e| anyhow!("Failed to build gossipsub config: {}", e))?;

        // Create gossipsub behaviour
        let gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        ).map_err(|e| anyhow!("Failed to create gossipsub: {}", e))?;

        // Create mDNS behaviour for local network discovery
        let mdns = if enable_mdns {
            mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)
                .map_err(|e| anyhow!("Failed to create mDNS: {}", e))?
        } else {
            mdns::tokio::Behaviour::new(
                mdns::Config {
                    enable_ipv6: false,
                    ..Default::default()
                },
                local_peer_id,
            ).map_err(|e| anyhow!("Failed to create mDNS: {}", e))?
        };

        // Create Kademlia DHT for peer discovery
        let store = MemoryStore::new(local_peer_id);
        let mut kademlia = if enable_dht {
            let mut kad = kad::Behaviour::new(local_peer_id, store);
            kad.set_mode(Some(kad::Mode::Server));
            kad
        } else {
            kad::Behaviour::new(local_peer_id, store)
        };

        // Add bootstrap nodes to Kademlia for global discovery
        if use_bootstrap && enable_dht {
            for addr in BOOTSTRAP_NODES {
                if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
                    if let Some(Protocol::P2p(peer_id)) = multiaddr.iter().last() {
                        let peer_id = peer_id.try_into();
                        if let Ok(peer_id) = peer_id {
                            kademlia.add_address(&peer_id, multiaddr);
                            info!("🌐 Added bootstrap node: {}", peer_id);
                        }
                    }
                }
            }
        }

        // Create relay behaviour for NAT traversal
        let relay = if relay_mode {
            relay::Behaviour::new(local_peer_id, relay::Config::default())
        } else {
            relay::Behaviour::new(local_peer_id, relay::Config::default())
        };

        // Create DCUtR behaviour for hole punching
        let dcutr = dcutr::Behaviour::new(local_peer_id);

        // Create AutoNAT behaviour for NAT detection
        let autonat = autonat::Behaviour::new(local_peer_id, autonat::Config::default());

        // Create identify behaviour
        let identify = identify::Behaviour::new(identify::Config::new(
            PROTOCOL_VERSION.to_string(),
            local_key.public(),
        ));

        // Create ping behaviour
        let ping = ping::Behaviour::new(ping::Config::new());

        // Combine all behaviours
        let behaviour = P2PBehaviour {
            gossipsub,
            mdns,
            kademlia,
            identify,
            ping,
            relay,
            dcutr,
            autonat,
        };

        // Create swarm with proper config
        let swarm_config = libp2p::swarm::Config::with_tokio_executor();
        let mut swarm = Swarm::new(transport, behaviour, local_peer_id, swarm_config);

        // Listen on specified port or random port
        let listen_addr = if let Some(port) = port {
            format!("/ip4/0.0.0.0/tcp/{}", port)
        } else {
            "/ip4/0.0.0.0/tcp/0".to_string()
        };

        swarm.listen_on(listen_addr.parse()
            .map_err(|e| anyhow!("Failed to parse listen address: {}", e))?)?;

        // Subscribe to handshake topic
        let handshake_topic = IdentTopic::new(HANDSHAKE_TOPIC);
        swarm.behaviour_mut().gossipsub.subscribe(&handshake_topic)?;

        info!("🎯 Subscribed to handshake topic: {}", HANDSHAKE_TOPIC);

        Ok(Self {
            swarm,
            node_name: name,
            handshake_topic,
        })
    }

    pub async fn bootstrap_global_network(&mut self) -> Result<()> {
        info!("🌐 Bootstrapping global network...");
        
        // Try to connect to bootstrap nodes
        for addr in BOOTSTRAP_NODES {
            if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
                info!("🔗 Connecting to bootstrap node: {}", multiaddr);
                if let Err(e) = self.swarm.dial(multiaddr.clone()) {
                    debug!("Failed to dial bootstrap node {}: {}", multiaddr, e);
                }
            }
        }

        // Start Kademlia bootstrap process
        if let Err(e) = self.swarm.behaviour_mut().kademlia.bootstrap() {
            debug!("Kademlia bootstrap failed: {}", e);
        }

        Ok(())
    }

    pub async fn connect_to_peer(&mut self, addr: &str) -> Result<()> {
        let multiaddr: Multiaddr = addr.parse()?;
        
        // Extract peer ID from multiaddr if present
        if let Some(Protocol::P2p(peer_id)) = multiaddr.iter().last() {
            info!("🔗 Connecting to peer: {} at {}", peer_id, multiaddr);
            self.swarm.dial(multiaddr)?;
        } else {
            return Err(anyhow!("Multiaddr must contain peer ID"));
        }
        
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("🌐 P2P node is running and ready to connect!");
        
        // Bootstrap the global network
        self.bootstrap_global_network().await?;
        
        let mut handshake_interval = interval(Duration::from_secs(30));
        let mut bootstrap_interval = interval(Duration::from_secs(300)); // Re-bootstrap every 5 minutes
        
        loop {
            select! {
                event = self.swarm.next() => {
                    if let Some(event) = event {
                        match event {
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Autonat(autonat::Event::StatusChanged { old, new })) => {
                                info!("🔍 NAT status changed from {:?} to {:?}", old, new);
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Dcutr(event)) => {
                                match event {
                                    dcutr::Event::InitiatedDirectConnectionUpgrade { remote_peer_id, .. } => {
                                        info!("🔄 Initiated direct connection upgrade to {}", remote_peer_id);
                                    }
                                    dcutr::Event::RemoteInitiatedDirectConnectionUpgrade { remote_peer_id, .. } => {
                                        info!("🔄 Remote initiated direct connection upgrade from {}", remote_peer_id);
                                    }
                                    dcutr::Event::DirectConnectionUpgradeSucceeded { remote_peer_id } => {
                                        info!("✅ Direct connection upgrade succeeded with {}", remote_peer_id);
                                    }
                                    dcutr::Event::DirectConnectionUpgradeFailed { remote_peer_id, error } => {
                                        warn!("❌ Direct connection upgrade failed with {}: {}", remote_peer_id, error);
                                    }
                                }
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Relay(relay::Event::ReservationReqAccepted { src_peer_id, .. })) => {
                                info!("🔗 Relay reservation accepted by {}", src_peer_id);
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                                for (peer_id, multiaddr) in list {
                                    info!("🔍 mDNS discovered peer: {} at {}", peer_id, multiaddr);
                                    
                                    // Add to Kademlia routing table
                                    self.swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr.clone());
                                    
                                    // Try to connect
                                    if let Err(e) = self.swarm.dial(multiaddr.clone()) {
                                        debug!("Failed to dial discovered peer {}: {}", peer_id, e);
                                    }
                                }
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                                for (peer_id, multiaddr) in list {
                                    debug!("📤 mDNS peer expired: {} at {}", peer_id, multiaddr);
                                }
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                                propagation_source: peer_id,
                                message,
                                ..
                            })) => {
                                if message.topic == self.handshake_topic.hash() {
                                    self.handle_handshake_message(peer_id, &message.data).await;
                                }
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Identify(identify::Event::Received {
                                peer_id,
                                info,
                            })) => {
                                info!("🆔 Identified peer: {} with protocol {}", peer_id, info.protocol_version);
                                
                                // Add addresses to Kademlia
                                for addr in info.listen_addrs {
                                    self.swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                                }
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
                                result: kad::QueryResult::Bootstrap(Ok(kad::BootstrapOk { num_remaining, .. })),
                                ..
                            })) => {
                                info!("🌐 DHT bootstrap progress: {} queries remaining", num_remaining);
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
                                result: kad::QueryResult::GetClosestPeers(Ok(kad::GetClosestPeersOk { key, peers, .. })),
                                ..
                            })) => {
                                info!("🔍 Found {} peers close to key", peers.len());
                                
                                // Try to connect to discovered peers
                                for peer in peers {
                                    if !self.swarm.is_connected(&peer) {
                                        if let Err(e) = self.swarm.dial(peer) {
                                            debug!("Failed to dial discovered peer {}: {}", peer, e);
                                        }
                                    }
                                }
                            }
                            
                            SwarmEvent::Behaviour(P2PBehaviourEvent::Ping(event)) => {
                                match event.result {
                                    Ok(rtt) => {
                                        debug!("🏓 Ping to {} successful: {:?}", event.peer, rtt);
                                    }
                                    Err(e) => {
                                        debug!("🏓 Ping to {} failed: {}", event.peer, e);
                                    }
                                }
                            }
                            
                            SwarmEvent::NewListenAddr { address, .. } => {
                                let local_peer_id = *self.swarm.local_peer_id();
                                info!("🎧 Listening on: {}/p2p/{}", address, local_peer_id);
                                
                                // Bootstrap the DHT after we start listening
                                if let Err(e) = self.swarm.behaviour_mut().kademlia.bootstrap() {
                                    debug!("Failed to bootstrap Kademlia: {}", e);
                                }
                                
                                // Start random walk to discover peers
                                let random_peer_id = PeerId::random();
                                self.swarm.behaviour_mut().kademlia.get_closest_peers(random_peer_id);
                            }
                            
                            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                                info!("🤝 Connected to peer: {}", peer_id);
                                self.send_handshake_message(peer_id).await;
                            }
                            
                            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                                info!("👋 Disconnected from peer: {}", peer_id);
                            }
                            
                            SwarmEvent::IncomingConnection { .. } => {
                                debug!("📞 Incoming connection");
                            }
                            
                            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                                if let Some(peer_id) = peer_id {
                                    warn!("❌ Outgoing connection error to {}: {}", peer_id, error);
                                } else {
                                    warn!("❌ Outgoing connection error: {}", error);
                                }
                            }
                            
                            SwarmEvent::IncomingConnectionError { error, .. } => {
                                warn!("❌ Incoming connection error: {}", error);
                            }
                            
                            _ => {}
                        }
                    }
                }
                
                _ = handshake_interval.tick() => {
                    self.broadcast_handshake().await;
                }
                
                _ = bootstrap_interval.tick() => {
                    // Periodically re-bootstrap and discover new peers
                    info!("🔄 Periodic network discovery...");
                    if let Err(e) = self.swarm.behaviour_mut().kademlia.bootstrap() {
                        debug!("Periodic bootstrap failed: {}", e);
                    }
                    
                    // Random walk to find new peers
                    let random_peer_id = PeerId::random();
                    self.swarm.behaviour_mut().kademlia.get_closest_peers(random_peer_id);
                }
            }
        }
    }

    async fn send_handshake_message(&mut self, peer_id: PeerId) {
        let handshake = HandshakeMessage {
            node_name: self.node_name.clone(),
            peer_id: self.swarm.local_peer_id().to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            message: format!(
                "Hello from {}! 👋",
                self.node_name.as_deref().unwrap_or("Anonymous Node")
            ),
        };

        if let Ok(message_json) = serde_json::to_string(&handshake) {
            if let Err(e) = self.swarm
                .behaviour_mut()
                .gossipsub
                .publish(self.handshake_topic.clone(), message_json.as_bytes())
            {
                error!("Failed to publish handshake message: {}", e);
            } else {
                info!("📤 Sent handshake to {}", peer_id);
            }
        }
    }

    async fn broadcast_handshake(&mut self) {
        let connected_peers: Vec<PeerId> = self.swarm.connected_peers().cloned().collect();
        
        if !connected_peers.is_empty() {
            info!("📡 Broadcasting handshake to {} connected peers", connected_peers.len());
            
            let handshake = HandshakeMessage {
                node_name: self.node_name.clone(),
                peer_id: self.swarm.local_peer_id().to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                message: format!(
                    "Periodic handshake from {}! Current time: {}",
                    self.node_name.as_deref().unwrap_or("Anonymous Node"),
                    chrono::Utc::now().format("%H:%M:%S")
                ),
            };

            if let Ok(message_json) = serde_json::to_string(&handshake) {
                if let Err(e) = self.swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(self.handshake_topic.clone(), message_json.as_bytes())
                {
                    error!("Failed to broadcast handshake: {}", e);
                }
            }
        }
    }

    async fn handle_handshake_message(&self, peer_id: PeerId, data: &[u8]) {
        match serde_json::from_slice::<HandshakeMessage>(data) {
            Ok(handshake) => {
                info!(
                    "🤝 Received handshake from {} ({}): {}",
                    peer_id,
                    handshake.node_name.as_deref().unwrap_or("Anonymous"),
                    handshake.message
                );
            }
            Err(e) => {
                warn!("Failed to parse handshake message: {}", e);
            }
        }
    }
}