use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures::StreamExt;
use libp2p::gossipsub::{
    self, AllowAllSubscriptionFilter, IdentTopic, IdentityTransform, MessageAuthenticity,
    ValidationMode,
};
use libp2p::kad::{self, store::MemoryStore, GetRecordOk, PutRecordOk, Quorum, Record, RecordKey};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{identity, noise, tcp, yamux, Multiaddr, PeerId, Swarm, Transport};
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};

use crate::error::{ButterflyBotError, Result as BotResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    pub kind: String,
    pub to: String,
    pub from: String,
    pub message_id: u64,
    pub payload: serde_json::Value,
    pub signature: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignableGossipMessage {
    pub kind: String,
    pub to: String,
    pub from: String,
    pub message_id: u64,
    pub payload: serde_json::Value,
}

type DhtGetResult = BotResult<Option<Vec<u8>>>;
type DhtPutResult = BotResult<()>;

enum GossipCommand {
    Publish(GossipMessage),
    Dial(Multiaddr),
    PutRecord {
        key: String,
        value: Vec<u8>,
        respond_to: oneshot::Sender<DhtPutResult>,
    },
    GetRecord {
        key: String,
        respond_to: oneshot::Sender<DhtGetResult>,
    },
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "AppBehaviourEvent")]
struct AppBehaviour {
    gossipsub: gossipsub::Behaviour<IdentityTransform, AllowAllSubscriptionFilter>,
    kademlia: kad::Behaviour<MemoryStore>,
}

enum AppBehaviourEvent {
    Gossipsub(gossipsub::Event),
    Kademlia(kad::Event),
}

impl From<gossipsub::Event> for AppBehaviourEvent {
    fn from(event: gossipsub::Event) -> Self {
        Self::Gossipsub(event)
    }
}

impl From<kad::Event> for AppBehaviourEvent {
    fn from(event: kad::Event) -> Self {
        Self::Kademlia(event)
    }
}

fn verify_message(message: &GossipMessage) -> BotResult<()> {
    if message.signature.trim().is_empty() || message.public_key.trim().is_empty() {
        return Err(ButterflyBotError::Runtime("missing signature".to_string()));
    }
    let signable = SignableGossipMessage {
        kind: message.kind.clone(),
        to: message.to.clone(),
        from: message.from.clone(),
        message_id: message.message_id,
        payload: message.payload.clone(),
    };
    let payload_bytes = serde_json::to_vec(&signable)
        .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
    let signature = BASE64
        .decode(message.signature.as_bytes())
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let public_key_bytes = BASE64
        .decode(message.public_key.as_bytes())
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let public_key = identity::PublicKey::try_decode_protobuf(&public_key_bytes)
        .map_err(|e: identity::DecodingError| ButterflyBotError::Runtime(e.to_string()))?;
    if !public_key.verify(&payload_bytes, &signature) {
        return Err(ButterflyBotError::Runtime("invalid signature".to_string()));
    }
    Ok(())
}

#[derive(Clone)]
pub struct GossipHandle {
    cmd_tx: mpsc::Sender<GossipCommand>,
    event_tx: broadcast::Sender<GossipMessage>,
    pub peer_id: PeerId,
    listen_addrs: Arc<RwLock<Vec<Multiaddr>>>,
    keypair: identity::Keypair,
}

impl GossipHandle {
    pub async fn start(
        listen_addrs: Vec<Multiaddr>,
        bootstrap: Vec<Multiaddr>,
        topic_name: &str,
    ) -> BotResult<Self> {
        let key_path = std::env::var("BUTTERFLY_BOT_GOSSIP_KEY_PATH")
            .unwrap_or_else(|_| "./data/gossip.key".to_string());
        let local_key = load_or_create_keypair(&key_path)?;
        let peer_id = PeerId::from(local_key.public());

        let transport = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true))
            .upgrade(libp2p::core::upgrade::Version::V1Lazy)
            .authenticate(
                noise::Config::new(&local_key)
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?,
            )
            .multiplex(yamux::Config::default())
            .boxed();

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validation_mode(ValidationMode::Strict)
            .heartbeat_interval(Duration::from_secs(10))
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let mut gossip = gossipsub::Behaviour::<IdentityTransform, AllowAllSubscriptionFilter>::new(
            MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let topic = IdentTopic::new(topic_name);
        gossip
            .subscribe(&topic)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let mut kademlia = kad::Behaviour::new(peer_id, MemoryStore::new(peer_id));
        kademlia.set_mode(Some(kad::Mode::Server));

        let behaviour = AppBehaviour {
            gossipsub: gossip,
            kademlia,
        };

        let mut swarm = Swarm::new(
            transport,
            behaviour,
            peer_id,
            libp2p::swarm::Config::with_tokio_executor(),
        );

        for addr in listen_addrs {
            swarm
                .listen_on(addr)
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }

        for addr in bootstrap {
            let _ = swarm.dial(addr);
        }

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<GossipCommand>(64);
        let (event_tx, _) = broadcast::channel::<GossipMessage>(256);
        let event_tx_task = event_tx.clone();
        let topic_task = topic.clone();
        let listen_addrs = Arc::new(RwLock::new(Vec::<Multiaddr>::new()));
        let listen_addrs_task = listen_addrs.clone();
        let mut pending_get: HashMap<kad::QueryId, oneshot::Sender<DhtGetResult>> = HashMap::new();
        let mut pending_put: HashMap<kad::QueryId, oneshot::Sender<DhtPutResult>> = HashMap::new();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(cmd) = cmd_rx.recv() => {
                        match cmd {
                            GossipCommand::Publish(message) => {
                                if let Ok(data) = serde_json::to_vec(&message) {
                                    let _ = swarm
                                        .behaviour_mut()
                                        .gossipsub
                                        .publish(topic_task.clone(), data);
                                }
                            }
                            GossipCommand::Dial(addr) => {
                                let _ = swarm.dial(addr);
                            }
                            GossipCommand::PutRecord { key, value, respond_to } => {
                                let record = Record {
                                    key: RecordKey::new(&key),
                                    value,
                                    publisher: None,
                                    expires: None,
                                };
                                match swarm.behaviour_mut().kademlia.put_record(record, Quorum::One) {
                                    Ok(query_id) => {
                                        pending_put.insert(query_id, respond_to);
                                    }
                                    Err(err) => {
                                        let _ = respond_to.send(Err(ButterflyBotError::Runtime(err.to_string())));
                                    }
                                }
                            }
                            GossipCommand::GetRecord { key, respond_to } => {
                                let query_id = swarm
                                    .behaviour_mut()
                                    .kademlia
                                    .get_record(RecordKey::new(&key));
                                pending_get.insert(query_id, respond_to);
                            }
                        }
                    }
                    event = swarm.select_next_some() => {
                        match event {
                            SwarmEvent::Behaviour(AppBehaviourEvent::Gossipsub(
                                gossipsub::Event::Message { message, .. },
                            )) => {
                                if let Ok(msg) = serde_json::from_slice::<GossipMessage>(&message.data) {
                                    if verify_message(&msg).is_ok() {
                                        let _ = event_tx_task.send(msg);
                                    }
                                }
                            }
                            SwarmEvent::Behaviour(AppBehaviourEvent::Kademlia(event)) => {
                                if let kad::Event::OutboundQueryProgressed { id, result, .. } = event {
                                    match result {
                                        kad::QueryResult::GetRecord(Ok(GetRecordOk::FoundRecord(record))) => {
                                            if let Some(sender) = pending_get.remove(&id) {
                                                let _ = sender.send(Ok(Some(record.record.value)));
                                            }
                                        }
                                        kad::QueryResult::GetRecord(Ok(GetRecordOk::FinishedWithNoAdditionalRecord { .. })) => {
                                            if let Some(sender) = pending_get.remove(&id) {
                                                let _ = sender.send(Ok(None));
                                            }
                                        }
                                        kad::QueryResult::GetRecord(Err(_)) => {
                                            if let Some(sender) = pending_get.remove(&id) {
                                                let _ = sender.send(Ok(None));
                                            }
                                        }
                                        kad::QueryResult::PutRecord(Ok(PutRecordOk { .. })) => {
                                            if let Some(sender) = pending_put.remove(&id) {
                                                let _ = sender.send(Ok(()));
                                            }
                                        }
                                        kad::QueryResult::PutRecord(Err(err)) => {
                                            if let Some(sender) = pending_put.remove(&id) {
                                                let _ = sender.send(Err(ButterflyBotError::Runtime(err.to_string())));
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SwarmEvent::NewListenAddr { address, .. } => {
                                let mut list: tokio::sync::RwLockWriteGuard<'_, Vec<Multiaddr>> =
                                    listen_addrs_task.write().await;
                                if !list.iter().any(|addr| addr == &address) {
                                    list.push(address);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(Self {
            cmd_tx,
            event_tx,
            peer_id,
            listen_addrs,
            keypair: local_key,
        })
    }

    pub async fn publish(&self, message: GossipMessage) -> BotResult<()> {
        let signable = SignableGossipMessage {
            kind: message.kind.clone(),
            to: message.to.clone(),
            from: message.from.clone(),
            message_id: message.message_id,
            payload: message.payload.clone(),
        };
        let payload_bytes = serde_json::to_vec(&signable)
            .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
        let signature = self
            .keypair
            .sign(&payload_bytes)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        let public_key = self
            .keypair
            .to_protobuf_encoding()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        let signed = GossipMessage {
            signature: BASE64.encode(signature),
            public_key: BASE64.encode(public_key),
            ..message
        };

        self.cmd_tx
            .send(GossipCommand::Publish(signed))
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
    }

    pub async fn dial(&self, addr: Multiaddr) -> BotResult<()> {
        self.cmd_tx
            .send(GossipCommand::Dial(addr))
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
    }

    pub async fn dht_put(&self, key: String, value: Vec<u8>) -> BotResult<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(GossipCommand::PutRecord {
                key,
                value,
                respond_to: tx,
            })
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        rx.await.map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
    }

    pub async fn dht_get(&self, key: String) -> BotResult<Option<Vec<u8>>> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(GossipCommand::GetRecord { key, respond_to: tx })
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        rx.await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<GossipMessage> {
        self.event_tx.subscribe()
    }

    pub async fn listen_addrs(&self) -> Vec<Multiaddr> {
        let list = self.listen_addrs.read().await;
        list.clone()
    }
}

fn load_or_create_keypair(path: &str) -> BotResult<identity::Keypair> {
    if let Some(parent) = Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(encoded) = fs::read_to_string(path) {
        if let Ok(raw) = BASE64.decode(encoded.trim()) {
            if let Ok(keypair) = identity::Keypair::from_protobuf_encoding(&raw) {
                return Ok(keypair);
            }
        }
    }

    let keypair = identity::Keypair::generate_ed25519();
    let encoded = keypair
        .to_protobuf_encoding()
        .map(|raw| BASE64.encode(raw))
        .unwrap_or_default();
    if !encoded.is_empty() {
        let _ = fs::write(path, encoded);
    }
    Ok(keypair)
}
