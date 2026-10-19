use crate::peer::peer_registry::PeerRegistry;
use crate::rlpx::message::{EthVersion, Hello, Pong, Status, StatusMessage};
use crate::rlpx::stream::RlpxStream;
use alloy_rlp::Decodable;
use anyhow::{anyhow, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wasix_eth_storage::read_traits::HeaderProvider;
use wasix_eth_types::p2p::{Disconnect, StatusEth69};
use wasix_eth_types::{BlockId, BlockNumberOrTag};

pub fn create_local_hello(registry: &PeerRegistry) -> Hello {
    Hello {
        protocol_version: 5,
        client_version: "wasix-eth/v0.1.0".to_string(),
        capabilities: vec![
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 64 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 65 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 66 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 67 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 68 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 69 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 70 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 71 },
            crate::rlpx::message::Capability { name: "eth".to_string(), version: 72 },
            crate::rlpx::message::Capability { name: "snap".to_string(), version: 1 },
        ],
        listen_port: registry.p2p_port(),
        id: registry.local_identity().public_key_b512(),
    }
}

pub async fn create_local_status<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    registry: &PeerRegistry,
    stream: &RlpxStream<S>,
) -> Result<StatusMessage> {
    let (head_hash, latest_height) = registry.chain_manager.head_block().await;
    
    wasix_eth_utils::info!("[P2P Handshake] Creating local status: head_hash={:?}, latest_height={}, genesis_hash={:?}", head_hash, latest_height, registry.genesis_hash);

    let head_td = registry.read_provider.header_td(head_hash).await.ok().flatten()
        .unwrap_or_default();

    let negotiated_version_u8 = stream.shared_capabilities.iter()
        .find(|c| c.name == "eth")
        .map(|c| c.version)
        .unwrap_or(68);
    
    let negotiated_version = EthVersion::try_from(negotiated_version_u8 as u8).unwrap_or(EthVersion::Eth68);

    let local_status = Status {
        version: negotiated_version,
        chain: registry.network_id,
        total_difficulty: head_td,
        blockhash: head_hash,
        genesis: registry.genesis_hash,
        forkid: registry.get_fork_id().await,
    };

    if negotiated_version >= EthVersion::Eth69 {
        Ok(StatusMessage::Eth69(StatusEth69 {
            version: negotiated_version,
            chain: registry.network_id,
            genesis: registry.genesis_hash,
            forkid: registry.get_fork_id().await,
            earliest: registry.read_provider.header(BlockId::Number(BlockNumberOrTag::Earliest)).await.ok().flatten().map(|h| h.number).unwrap_or(0),
            latest: latest_height,
            blockhash: head_hash,
        }))
    } else {
        Ok(StatusMessage::Legacy(local_status))
    }
}

pub async fn do_p2p_handshake<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut RlpxStream<S>,
    local_hello: Hello,
) -> Result<()> {
    // In RLPx, both sides can send Hello immediately after ECIES handshake.
    stream.send_p2p(&local_hello, 0x00).await?;

    let mut temp_buffer = Vec::new();
    let remote_hello = loop {
        let (id, payload) = stream.read_message().await?;
        match id {
            0x00 => break Hello::decode(&mut &payload[..])?,
            0x01 => {
                let disconnect = Disconnect::decode(&mut &payload[..])?;
                return Err(anyhow!("Received disconnect during handshake: reason={}", disconnect.reason));
            }
            0x02 => { // Ping
                stream.send_p2p(&Pong {}, 0x03).await?;
            }
            0x03 => continue, // Pong
            _ if id >= 0x10 => {
                // Some peers might send subprotocol messages (like eth Status) immediately after ECIES,
                // or even before we send/receive Hello. Buffer them locally first.
                temp_buffer.push((id, payload));
            }
            _ => return Err(anyhow!("Expected Hello (0x00), got {}", id)),
        }
    };

    // Restore buffered messages to the stream's buffer in the correct order
    for msg in temp_buffer.into_iter().rev() {
        stream.msg_buffer.push_front(msg);
    }

    stream.remote_id = Some(remote_hello.id);
    stream.remote_client_version = Some(remote_hello.client_version.clone());
    
    // Negotiate capabilities and assign offsets
    use std::collections::{BTreeSet, HashMap};
    let mut shared_capabilities_map: HashMap<String, u64> = HashMap::new();
    let mut shared_capability_names = BTreeSet::new();

    for local_cap in &local_hello.capabilities {
        for remote_cap in &remote_hello.capabilities {
            if local_cap.name == remote_cap.name {
                let entry = shared_capabilities_map.entry(local_cap.name.clone()).or_insert(0);
                if remote_cap.version <= local_cap.version && remote_cap.version > *entry {
                    *entry = remote_cap.version;
                    shared_capability_names.insert(local_cap.name.clone());
                } else if remote_cap.version > local_cap.version {
                    // If remote has higher version, we use our highest supported version
                    if local_cap.version > *entry {
                        *entry = local_cap.version;
                        shared_capability_names.insert(local_cap.name.clone());
                    }
                }
            }
        }
    }

    if shared_capability_names.is_empty() {
        return Err(anyhow!("No common capabilities found with peer"));
    }

    let mut offset = 0x10;
    for name in shared_capability_names {
        let version = shared_capabilities_map[&name];
        stream.shared_capabilities.push(crate::rlpx::stream::SharedCapability {
            name: name.clone(),
            version: version as u8,
            offset,
        });
        
        if name == "eth" {
            let ev = EthVersion::try_from(version as u8).map_err(|_| anyhow!("Invalid eth version"))?;
            offset += ev.message_count();
        } else if name == "snap" {
            offset += 8; // EIP-2364: Snap protocol defines 8 messages
        } else {
            offset += 16;
        }
    }
    
    Ok(())
}

pub async fn do_eth_handshake<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    registry: &PeerRegistry,
    stream: &mut RlpxStream<S>,
    local_status: StatusMessage,
) -> Result<StatusMessage> {
    let (offset, version) = {
        let eth_cap = stream.shared_capabilities.iter().find(|c| c.name == "eth")
            .ok_or_else(|| anyhow!("Eth capability not negotiated"))?;
        (eth_cap.offset, eth_cap.version)
    };
    
    stream.send_eth(&local_status, 0x00).await?;
    
    let mut temp_buffer = Vec::new();
    let (_id, payload) = loop {
        let (id, payload) = stream.read_message().await?;
        if id == offset {
            break (id, payload);
        }
        match id {
            0x01 => {
                let disconnect = Disconnect::decode(&mut &payload[..])?;
                return Err(anyhow!("Received disconnect during eth handshake: reason={}", disconnect.reason));
            }
            0x02 => { // Ping
                stream.send_p2p(&Pong {}, 0x03).await?;
            }
            0x03 => continue, // Pong
            _ if id >= 0x10 => {
                temp_buffer.push((id, payload));
            }
            _ => {
                wasix_eth_utils::debug!("[Eth Handshake] Expected Eth Status ({}), got {}. Payload size: {}", offset, id, payload.len());
                return Err(anyhow!("Expected Eth Status ({}), got {}", offset, id));
            }
        }
    };

    // Restore buffered messages to the stream's buffer in the correct order
    for msg in temp_buffer.into_iter().rev() {
        stream.msg_buffer.push_front(msg);
    }
    
    let eth_version = EthVersion::try_from(version).map_err(|_| anyhow!("Invalid eth version"))?;
    let remote_status = match eth_version {
        EthVersion::Eth64 | EthVersion::Eth65 | EthVersion::Eth66 | EthVersion::Eth67 | EthVersion::Eth68 => {
            StatusMessage::Legacy(Status::decode(&mut &payload[..])?)
        }
        _ => {
            match StatusEth69::decode(&mut &payload[..]) {
                Ok(s) => StatusMessage::Eth69(s),
                Err(e) => {
                    wasix_eth_utils::debug!("[Eth Handshake] Failed to decode StatusEth69: {}. Trying legacy Status.", e);
                    StatusMessage::Legacy(Status::decode(&mut &payload[..])?)
                }
            }
        }
    };

    // EIP-2364 / EIP-2124: Validate ForkId
    let remote_fork_id = match &remote_status {
        StatusMessage::Legacy(s) => &s.forkid,
        StatusMessage::Eth69(s) => &s.forkid,
    };
    
    let remote_genesis = match &remote_status {
        StatusMessage::Legacy(s) => s.genesis,
        StatusMessage::Eth69(s) => s.genesis,
    };

    if remote_genesis != registry.genesis_hash {
        return Err(anyhow!("Remote genesis hash mismatch: expected {}, got {}", registry.genesis_hash, remote_genesis));
    }

    let (head_hash, head_num) = registry.chain_manager.head_block().await;
    let head = registry.read_provider.header(BlockId::Hash(head_hash.into())).await.ok().flatten();
    let head_time = head.as_ref().map(|h| h.timestamp).unwrap_or(0);

    let genesis = registry.read_provider.header(BlockId::Hash(registry.genesis_hash.into())).await.ok().flatten();
    let genesis_time = genesis.as_ref().map(|h| h.timestamp).unwrap_or(0);

    if let Err(e) = remote_fork_id.validate(registry.genesis_hash, &registry.chain_config, head_num, head_time, genesis_time) {
        return Err(anyhow!("ForkId validation failed: {}", e));
    }

    Ok(remote_status)
}
