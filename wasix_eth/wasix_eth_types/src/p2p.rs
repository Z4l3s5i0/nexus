use alloy_primitives::{B256, B512, U256, Bytes};
use alloy_rlp::{RlpDecodable, RlpEncodable, Header, Buf, Decodable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EthVersion {
    Eth64 = 64,
    Eth65 = 65,
    Eth66 = 66,
    Eth67 = 67,
    Eth68 = 68,
    Eth69 = 69,
    Eth70 = 70,
    Eth71 = 71,
    Eth72 = 72,
}

impl EthVersion {
    pub fn message_count(&self) -> u8 {
        EthMessageID::message_count(*self)
    }

    pub const fn has_request_id(&self) -> bool {
        *self as u8 >= EthVersion::Eth66 as u8
    }

    pub const fn is_eth72(&self) -> bool {
        *self as u8 >= EthVersion::Eth72 as u8
    }

    pub const fn is_eth71(&self) -> bool {
        *self as u8 >= EthVersion::Eth71 as u8
    }

    pub const fn is_eth69_or_newer(&self) -> bool {
        *self as u8 >= EthVersion::Eth69 as u8
    }

    pub const fn is_eth68_or_newer(&self) -> bool {
        *self as u8 >= EthVersion::Eth68 as u8
    }

    pub const fn supports_get_node_data(&self) -> bool {
        (*self as u8) < (EthVersion::Eth67 as u8)
    }

    pub const fn supports_receipts(&self) -> bool {
        true
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EthMessageID {
    Status = 0x00,
    NewBlockHashes = 0x01,
    Transactions = 0x02,
    GetBlockHeaders = 0x03,
    BlockHeaders = 0x04,
    GetBlockBodies = 0x05,
    BlockBodies = 0x06,
    NewBlock = 0x07,
    NewPooledTransactionHashes = 0x08,
    GetPooledTransactions = 0x09,
    PooledTransactions = 0x0a,
    GetNodeData = 0x0d,
    NodeData = 0x0e,
    GetReceipts = 0x0f,
    Receipts = 0x10,
    BlockRangeUpdate = 0x11,
    GetBlockAccessLists = 0x12,
    BlockAccessLists = 0x13,
    GetCells = 0x14,
    Cells = 0x15,
    Other(u8),
}

impl EthMessageID {
    pub const fn to_u8(&self) -> u8 {
        match self {
            Self::Status => 0x00,
            Self::NewBlockHashes => 0x01,
            Self::Transactions => 0x02,
            Self::GetBlockHeaders => 0x03,
            Self::BlockHeaders => 0x04,
            Self::GetBlockBodies => 0x05,
            Self::BlockBodies => 0x06,
            Self::NewBlock => 0x07,
            Self::NewPooledTransactionHashes => 0x08,
            Self::GetPooledTransactions => 0x09,
            Self::PooledTransactions => 0x0a,
            Self::GetNodeData => 0x0d,
            Self::NodeData => 0x0e,
            Self::GetReceipts => 0x0f,
            Self::Receipts => 0x10,
            Self::BlockRangeUpdate => 0x11,
            Self::GetBlockAccessLists => 0x12,
            Self::BlockAccessLists => 0x13,
            Self::GetCells => 0x14,
            Self::Cells => 0x15,
            Self::Other(value) => *value,
        }
    }

    pub const fn max(version: EthVersion) -> u8 {
        if version.is_eth72() {
            Self::Cells.to_u8()
        } else if version.is_eth71() {
            Self::BlockAccessLists.to_u8()
        } else if version.is_eth69_or_newer() {
            Self::BlockRangeUpdate.to_u8()
        } else {
            Self::Receipts.to_u8()
        }
    }

    pub const fn message_count(version: EthVersion) -> u8 {
        Self::max(version) + 1
    }
}

impl alloy_rlp::Encodable for EthMessageID {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_u8(self.to_u8());
    }
    fn length(&self) -> usize {
        1
    }
}

impl alloy_rlp::Decodable for EthMessageID {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = match buf.first().ok_or(alloy_rlp::Error::InputTooShort)? {
            0x00 => Self::Status,
            0x01 => Self::NewBlockHashes,
            0x02 => Self::Transactions,
            0x03 => Self::GetBlockHeaders,
            0x04 => Self::BlockHeaders,
            0x05 => Self::GetBlockBodies,
            0x06 => Self::BlockBodies,
            0x07 => Self::NewBlock,
            0x08 => Self::NewPooledTransactionHashes,
            0x09 => Self::GetPooledTransactions,
            0x0a => Self::PooledTransactions,
            0x0d => Self::GetNodeData,
            0x0e => Self::NodeData,
            0x0f => Self::GetReceipts,
            0x10 => Self::Receipts,
            0x11 => Self::BlockRangeUpdate,
            0x12 => Self::GetBlockAccessLists,
            0x13 => Self::BlockAccessLists,
            0x14 => Self::GetCells,
            0x15 => Self::Cells,
            unknown => Self::Other(*unknown),
        };
        buf.advance(1);
        Ok(id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestPair<T> {
    pub request_id: u64,
    pub message: T,
}

impl<T> alloy_rlp::Encodable for RequestPair<T>
where
    T: alloy_rlp::Encodable,
{
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let header = Header {
            list: true,
            payload_length: self.request_id.length() + self.message.length(),
        };
        header.encode(out);
        self.request_id.encode(out);
        self.message.encode(out);
    }
    fn length(&self) -> usize {
        let payload_len = self.request_id.length() + self.message.length();
        alloy_rlp::length_of_length(payload_len) + payload_len
    }
}

impl<T> alloy_rlp::Decodable for RequestPair<T>
where
    T: alloy_rlp::Decodable,
{
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let initial_len = buf.len();
        let request_id = u64::decode(buf)?;
        let message = T::decode(buf)?;
        let consumed = initial_len - buf.len();
        if consumed != header.payload_length {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(Self { request_id, message })
    }
}

impl alloy_rlp::Encodable for EthVersion {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        (*self as u8).encode(out)
    }
    fn length(&self) -> usize {
        (*self as u8).length()
    }
}

impl TryFrom<u8> for EthVersion {
    type Error = alloy_rlp::Error;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            64 => Ok(EthVersion::Eth64),
            65 => Ok(EthVersion::Eth65),
            66 => Ok(EthVersion::Eth66),
            67 => Ok(EthVersion::Eth67),
            68 => Ok(EthVersion::Eth68),
            69 => Ok(EthVersion::Eth69),
            70 => Ok(EthVersion::Eth70),
            71 => Ok(EthVersion::Eth71),
            72 => Ok(EthVersion::Eth72),
            _ => Err(alloy_rlp::Error::Custom("Unknown eth version")),
        }
    }
}

impl alloy_rlp::Decodable for EthVersion {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let v = u8::decode(buf)?;
        EthVersion::try_from(v)
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone)]
pub struct Hello {
    pub protocol_version: u64,
    pub client_version: String,
    pub capabilities: Vec<Capability>,
    pub listen_port: u16,
    pub id: B512,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub version: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisconnectReason {
    #[default]
    DisconnectRequested = 0x00,
    TcpSubsystemError = 0x01,
    ProtocolBreach = 0x02,
    UselessPeer = 0x03,
    TooManyPeers = 0x04,
    AlreadyConnected = 0x05,
    IncompatibleP2PProtocolVersion = 0x06,
    NullNodeIdentity = 0x07,
    ClientQuitting = 0x08,
    UnexpectedHandshakeIdentity = 0x09,
    ConnectedToSelf = 0x0a,
    PingTimeout = 0x0b,
    SubprotocolSpecific = 0x10,
    Unknown = 0xff,
}

impl alloy_rlp::Encodable for DisconnectReason {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        alloy_rlp::encode_list(&[*self as u8], out);
    }
    fn length(&self) -> usize {
        alloy_rlp::list_length(&[*self as u8])
    }
}

impl alloy_rlp::Decodable for DisconnectReason {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort)
        }
        
        // Handle both single byte and single-element list
        let first = buf[0];
        if first >= 0x80 {
            // It's a list (or at least looks like one)
            let header = alloy_rlp::Header::decode(buf)?;
            if !header.list {
                return Err(alloy_rlp::Error::UnexpectedString);
            }
            if header.payload_length == 0 {
                 return Ok(Self::DisconnectRequested);
            }
        }
        
        let reason_code = u8::decode(buf)?;
        Ok(Self::from(reason_code as u64))
    }
}

impl From<u64> for DisconnectReason {
    fn from(value: u64) -> Self {
        match value {
            0x00 => Self::DisconnectRequested,
            0x01 => Self::TcpSubsystemError,
            0x02 => Self::ProtocolBreach,
            0x03 => Self::UselessPeer,
            0x04 => Self::TooManyPeers,
            0x05 => Self::AlreadyConnected,
            0x06 => Self::IncompatibleP2PProtocolVersion,
            0x07 => Self::NullNodeIdentity,
            0x08 => Self::ClientQuitting,
            0x09 => Self::UnexpectedHandshakeIdentity,
            0x0a => Self::ConnectedToSelf,
            0x0b => Self::PingTimeout,
            0x10 => Self::SubprotocolSpecific,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::DisconnectRequested => "disconnect requested",
            Self::TcpSubsystemError => "TCP sub-system error",
            Self::ProtocolBreach => "breach of protocol",
            Self::UselessPeer => "useless peer",
            Self::TooManyPeers => "too many peers",
            Self::AlreadyConnected => "already connected",
            Self::IncompatibleP2PProtocolVersion => "incompatible P2P protocol version",
            Self::NullNodeIdentity => "null node identity received",
            Self::ClientQuitting => "client quitting",
            Self::UnexpectedHandshakeIdentity => "unexpected identity in handshake",
            Self::ConnectedToSelf => "identity is the same as this node",
            Self::PingTimeout => "ping timeout",
            Self::SubprotocolSpecific => "some other reason specific to a subprotocol",
            Self::Unknown => "unknown",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone)]
pub struct Disconnect {
    pub reason: DisconnectReason,
}

impl alloy_rlp::Encodable for Disconnect {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.reason.encode(out);
    }
    fn length(&self) -> usize {
        self.reason.length()
    }
}

impl alloy_rlp::Decodable for Disconnect {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self { reason: DisconnectReason::decode(buf)? })
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone)]
pub struct Ping {}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone)]
pub struct Pong {}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct BlockHashAndNumber {
    pub hash: B256,
    pub number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewBlockHashes(pub Vec<BlockHashAndNumber>);

impl alloy_rlp::Encodable for NewBlockHashes {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for NewBlockHashes {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transactions(pub Vec<alloy_consensus::TxEnvelope>);

impl alloy_rlp::Encodable for Transactions {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for Transactions {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub version: EthVersion,
    pub chain: u64,
    pub total_difficulty: U256,
    pub blockhash: B256,
    pub genesis: B256,
    pub forkid: ForkId,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct StatusEth69 {
    pub version: EthVersion,
    pub chain: u64,
    pub genesis: B256,
    pub forkid: ForkId,
    pub earliest: u64,
    pub latest: u64,
    pub blockhash: B256,
}

#[derive(Debug, Clone)]
pub enum StatusMessage {
    Legacy(Status),
    Eth69(StatusEth69),
}

impl alloy_rlp::Encodable for StatusMessage {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            StatusMessage::Legacy(s) => s.encode(out),
            StatusMessage::Eth69(s) => s.encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            StatusMessage::Legacy(s) => s.length(),
            StatusMessage::Eth69(s) => s.length(),
        }
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct ForkId {
    pub hash: [u8; 4],
    pub next: u64,
}

impl ForkId {
    pub fn new(genesis_hash: B256, config: &alloy_genesis::ChainConfig, head_num: u64, head_time: u64, genesis_time: u64) -> Self {
        use crc::{Crc, CRC_32_ISO_HDLC};
        let crc = Crc::<u32>::new(&CRC_32_ISO_HDLC);
        let mut digest = crc.digest();
        digest.update(genesis_hash.as_slice());

        let mut forks = Vec::new();
        if let Some(b) = config.homestead_block { forks.push((b, false)); }
        if let Some(b) = config.dao_fork_block { forks.push((b, false)); }
        if let Some(b) = config.eip150_block { forks.push((b, false)); }
        if let Some(b) = config.eip155_block { forks.push((b, false)); }
        if let Some(b) = config.eip158_block { forks.push((b, false)); }
        if let Some(b) = config.byzantium_block { forks.push((b, false)); }
        if let Some(b) = config.constantinople_block { forks.push((b, false)); }
        if let Some(b) = config.petersburg_block { forks.push((b, false)); }
        if let Some(b) = config.istanbul_block { forks.push((b, false)); }
        if let Some(b) = config.muir_glacier_block { forks.push((b, false)); }
        if let Some(b) = config.berlin_block { forks.push((b, false)); }
        if let Some(b) = config.london_block { forks.push((b, false)); }
        if let Some(b) = config.arrow_glacier_block { forks.push((b, false)); }
        if let Some(b) = config.gray_glacier_block { forks.push((b, false)); }
        if let Some(b) = config.merge_netsplit_block { forks.push((b, false)); }
        if let Some(t) = config.shanghai_time { forks.push((t, true)); }
        if let Some(t) = config.cancun_time { forks.push((t, true)); }
        if let Some(t) = config.prague_time { forks.push((t, true)); }

        // EIP-6122: Sort by type first (blocks then timestamps), then by value
        forks.sort_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)));

        let mut next = 0;
        let mut last_added = 0;
        for (fork_val, is_timestamp) in forks {
            let active = if is_timestamp {
                fork_val <= head_time
            } else {
                fork_val <= head_num
            };

            if active {
                // EIP-2124/6122: Skip genesis activation points and duplicates
                let is_genesis = if is_timestamp {
                    fork_val <= genesis_time
                } else {
                    fork_val == 0
                };

                if !is_genesis && fork_val != last_added {
                    digest.update(&fork_val.to_be_bytes());
                    last_added = fork_val;
                }
            } else {
                next = fork_val;
                break;
            }
        }

        Self {
            hash: digest.finalize().to_be_bytes(),
            next,
        }
    }

    pub fn validate(&self, genesis_hash: B256, config: &alloy_genesis::ChainConfig, head_num: u64, head_time: u64, genesis_time: u64) -> Result<(), String> {
        use crc::{Crc, CRC_32_ISO_HDLC};
        let crc = Crc::<u32>::new(&CRC_32_ISO_HDLC);

        let mut forks = Vec::new();
        if let Some(b) = config.homestead_block { forks.push((b, false)); }
        if let Some(b) = config.dao_fork_block { forks.push((b, false)); }
        if let Some(b) = config.eip150_block { forks.push((b, false)); }
        if let Some(b) = config.eip155_block { forks.push((b, false)); }
        if let Some(b) = config.eip158_block { forks.push((b, false)); }
        if let Some(b) = config.byzantium_block { forks.push((b, false)); }
        if let Some(b) = config.constantinople_block { forks.push((b, false)); }
        if let Some(b) = config.petersburg_block { forks.push((b, false)); }
        if let Some(b) = config.istanbul_block { forks.push((b, false)); }
        if let Some(b) = config.muir_glacier_block { forks.push((b, false)); }
        if let Some(b) = config.berlin_block { forks.push((b, false)); }
        if let Some(b) = config.london_block { forks.push((b, false)); }
        if let Some(b) = config.arrow_glacier_block { forks.push((b, false)); }
        if let Some(b) = config.gray_glacier_block { forks.push((b, false)); }
        if let Some(b) = config.merge_netsplit_block { forks.push((b, false)); }
        if let Some(t) = config.shanghai_time { forks.push((t, true)); }
        if let Some(t) = config.cancun_time { forks.push((t, true)); }
        if let Some(t) = config.prague_time { forks.push((t, true)); }

        forks.sort_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)));

        let mut digest = crc.digest();
        digest.update(genesis_hash.as_slice());

        // We will track the local hash at every unique fork increment
        let mut local_hashes = Vec::new();
        // Gather genesis hash state
        local_hashes.push((digest.clone().finalize(), 0));

        let mut last_added = 0;
        for (fork_val, is_timestamp) in &forks {
            let is_genesis = if *is_timestamp { *fork_val <= genesis_time } else { *fork_val == 0 };
            if !is_genesis && *fork_val != last_added {
                digest.update(&fork_val.to_be_bytes());
                last_added = *fork_val;
                local_hashes.push((digest.clone().finalize(), *fork_val));
            }
        }

        // Find our current active fork position based on local block/time
        let mut current_local_idx = 0;
        for &(val, is_timestamp) in forks.iter() {
            let active = if is_timestamp { val <= head_time } else { val <= head_num };
            if active {
                // Find how many unique changes were applied up to this active fork
                current_local_idx = local_hashes.iter().rposition(|&(_, v)| v <= val).unwrap_or(0);
            }
        }

        let remote_hash = u32::from_be_bytes(self.hash);

        // Look for the remote's hash in our history timeline
        if let Some(remote_idx_in_local) = local_hashes.iter().position(|&(h, _)| h == remote_hash) {
            if remote_idx_in_local == current_local_idx {
                // Scenario A: Remote is on the exact same fork state as us.
                // Their next announced fork must match our next planned fork block/time.
                let next_local_fork = local_hashes.get(current_local_idx + 1).map(|&(_, v)| v).unwrap_or(0);
                if self.next != next_local_fork {
                    return Err(format!("Remote announced next fork {}, but local expects {}", self.next, next_local_fork));
                }
                return Ok(());
            } else if remote_idx_in_local < current_local_idx {
                // Scenario B: Remote is in our past (behind us).
                // Their next fork must be the one that transitioned us to the next state.
                let remote_expected_next = local_hashes.get(remote_idx_in_local + 1).map(|&(_, v)| v).unwrap_or(0);
                if self.next != remote_expected_next {
                    return Err(format!("Remote is behind and announced next fork {}, expected {}", self.next, remote_expected_next));
                }
                return Ok(());
            } else {
                // Scenario C: Remote is in our future (they have forks we know about but haven't activated yet).
                // Validate that we didn't miss a fork they already passed.
                let local_expected_next = local_hashes.get(current_local_idx + 1).map(|&(_, v)| v).unwrap_or(0);
                if local_expected_next != 0 && self.next == local_expected_next {
                    return Err("Remote bypassed a local fork activation".to_string());
                }
                return Ok(());
            }
        }

        Err("Remote has an incompatible fork history entirely".to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockHashOrNumber {
    Hash(B256),
    Number(u64),
}

impl BlockHashOrNumber {
    pub fn as_number(&self) -> Option<u64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }
}

impl alloy_rlp::Encodable for BlockHashOrNumber {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            BlockHashOrNumber::Hash(h) => h.encode(out),
            BlockHashOrNumber::Number(n) => n.encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            BlockHashOrNumber::Hash(h) => h.length(),
            BlockHashOrNumber::Number(n) => n.length(),
        }
    }
}

impl alloy_rlp::Decodable for BlockHashOrNumber {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort);
        }
        let b = *buf;
        if b[0] == 0xa0 {
            if let Ok(h) = B256::decode(buf) {
                return Ok(BlockHashOrNumber::Hash(h));
            }
        }
        u64::decode(buf).map(BlockHashOrNumber::Number)
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct GetBlockHeaders {
    pub block: BlockHashOrNumber,
    pub amount: u64,
    pub skip: u64,
    pub reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockHeaders(pub Vec<crate::Header>);

impl alloy_rlp::Encodable for BlockHeaders {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for BlockHeaders {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetBlockBodies(pub Vec<B256>);

impl alloy_rlp::Encodable for GetBlockBodies {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for GetBlockBodies {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockBodies(pub Vec<crate::BlockBody<crate::TxEnvelope>>);

impl alloy_rlp::Encodable for BlockBodies {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for BlockBodies {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetPooledTransactions(pub Vec<B256>);

impl alloy_rlp::Encodable for GetPooledTransactions {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for GetPooledTransactions {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PooledTransactions(pub Vec<alloy_consensus::TxEnvelope>);

impl alloy_rlp::Encodable for PooledTransactions {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for PooledTransactions {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Vec::<alloy_consensus::TxEnvelope>::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetReceipts(pub Vec<B256>);

impl alloy_rlp::Encodable for GetReceipts {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for GetReceipts {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Receipts(pub Vec<Vec<crate::consensus::Receipt>>);

impl alloy_rlp::Encodable for Receipts {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for Receipts {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Vec::<Vec<crate::consensus::Receipt>>::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetNodeData(pub Vec<B256>);

impl alloy_rlp::Encodable for GetNodeData {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for GetNodeData {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeData(pub Vec<Bytes>);

impl alloy_rlp::Encodable for NodeData {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out);
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl alloy_rlp::Decodable for NodeData {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self(Decodable::decode(buf)?))
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockRangeUpdate {
    pub earliest: u64,
    pub latest: u64,
    pub latest_hash: B256,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewBlock {
    pub block: alloy_consensus::Block<alloy_consensus::TxEnvelope>,
    pub total_difficulty: alloy_primitives::U256,
}

impl alloy_rlp::Encodable for NewBlock {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let list_header = Header {
            list: true,
            payload_length: self.block.length() + self.total_difficulty.length(),
        };
        list_header.encode(out);
        self.block.encode(out);
        self.total_difficulty.encode(out);
    }
    fn length(&self) -> usize {
        let payload_length = self.block.length() + self.total_difficulty.length();
        Header {
            list: true,
            payload_length,
        }.length() + payload_length
    }
}

impl alloy_rlp::Decodable for NewBlock {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let block = alloy_consensus::Block::decode(buf)?;
        let total_difficulty = alloy_primitives::U256::decode(buf)?;
        Ok(Self {
            block,
            total_difficulty,
        })
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct NewPooledTransactionHashes {
    pub types: Bytes,
    pub sizes: Vec<u64>,
    pub hashes: Vec<B256>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewPooledTransactionHashes66 {
    pub hashes: Vec<B256>,
}

impl alloy_rlp::Encodable for NewPooledTransactionHashes66 {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.hashes.encode(out);
    }
    fn length(&self) -> usize {
        self.hashes.length()
    }
}

impl alloy_rlp::Decodable for NewPooledTransactionHashes66 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self { hashes: Decodable::decode(buf)? })
    }
}

#[derive(Debug, Clone)]
pub enum GossipMessage {
    NewBlockHashes(String, u64, NewBlockHashes),
    Transactions(String, u64, Transactions),
    NewBlock(String, u64, NewBlock),
    NewPooledTransactionHashes(String, u64, NewPooledTransactionHashes),
    // Incoming requests from peers
    GetBlockHeaders(String, u64, RequestPair<GetBlockHeaders>),
    GetBlockBodies(String, u64, RequestPair<GetBlockBodies>),
    GetPooledTransactions(String, u64, RequestPair<GetPooledTransactions>),
    GetReceipts(String, u64, RequestPair<GetReceipts>),
    GetNodeData(String, u64, RequestPair<GetNodeData>),
}

/// Snap protocol message IDs (EIP-2364)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapMessageID {
    GetAccountRange = 0x00,
    AccountRange = 0x01,
    GetStorageRanges = 0x02,
    StorageRanges = 0x03,
    GetByteCodes = 0x04,
    ByteCodes = 0x05,
    GetTrieNodes = 0x06,
    TrieNodes = 0x07,
}

impl SnapMessageID {
    pub fn to_u8(&self) -> u8 {
        *self as u8
    }

    pub fn message_count() -> u8 {
        8
    }
}

impl alloy_rlp::Encodable for SnapMessageID {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        (self.to_u8() as u64).encode(out);
    }
    fn length(&self) -> usize {
        (self.to_u8() as u64).length()
    }
}

impl alloy_rlp::Decodable for SnapMessageID {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = u8::decode(buf)?;
        match id {
            0x00 => Ok(SnapMessageID::GetAccountRange),
            0x01 => Ok(SnapMessageID::AccountRange),
            0x02 => Ok(SnapMessageID::GetStorageRanges),
            0x03 => Ok(SnapMessageID::StorageRanges),
            0x04 => Ok(SnapMessageID::GetByteCodes),
            0x05 => Ok(SnapMessageID::ByteCodes),
            0x06 => Ok(SnapMessageID::GetTrieNodes),
            0x07 => Ok(SnapMessageID::TrieNodes),
            _ => Err(alloy_rlp::Error::Custom("Invalid SnapMessageID")),
        }
    }
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct GetAccountRange {
    pub request_id: u64,
    pub root: B256,
    pub origin: B256,
    pub limit: B256,
    pub response_bytes: u64,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct AccountRange {
    pub request_id: u64,
    pub accounts: Vec<AccountData>,
    pub proof: Vec<Bytes>,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct AccountData {
    pub hash: B256,
    pub body: Bytes,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct GetStorageRanges {
    pub request_id: u64,
    pub root: B256,
    pub accounts: Vec<B256>,
    pub origin: Bytes,
    pub limit: Bytes,
    pub response_bytes: u64,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct StorageRanges {
    pub request_id: u64,
    pub slots: Vec<Vec<SlotData>>,
    pub proof: Vec<Bytes>,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct SlotData {
    pub hash: B256,
    pub value: Bytes,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct GetByteCodes {
    pub request_id: u64,
    pub hashes: Vec<B256>,
    pub response_bytes: u64,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct ByteCodes {
    pub request_id: u64,
    pub codes: Vec<Bytes>,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct GetTrieNodes {
    pub request_id: u64,
    pub root: B256,
    pub paths: Vec<Vec<Bytes>>,
    pub response_bytes: u64,
}

#[derive(RlpEncodable, RlpDecodable, Debug, Clone, PartialEq, Eq)]
pub struct TrieNodes {
    pub request_id: u64,
    pub nodes: Vec<Bytes>,
}
