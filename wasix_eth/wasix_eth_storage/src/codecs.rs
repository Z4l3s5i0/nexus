use wasix_eth_types::{
    Header, B256, Address, U256, Receipt, ReceiptMeta, Transaction, Bytes, TrieAccount, PayloadId,
    Block, PeerEntry, BlobsBundleV1, BlockBody,
};
use crate::tables::*;
use std::fmt::Debug;
use alloy_rlp::{Decodable, Encodable};

use redb::{Value, TableDefinition, Key};

pub trait Table: Debug {
    const NAME: &'static str;
    type Key: RedbRlp + 'static;
    type Value: RedbRlp + 'static;

    fn definition() -> TableDefinition<'static, RlpValue<Self::Key>, RlpValue<Self::Value>> {
        TableDefinition::new(Self::NAME)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RlpValue<T>(pub T);

impl<T> std::borrow::Borrow<T> for RlpValue<T> {
    fn borrow(&self) -> &T {
        &self.0
    }
}

pub trait RedbRlp: Debug + 'static {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut);
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> where Self: Sized;
    fn rlp_length(&self) -> usize;
}

macro_rules! impl_redb_rlp {
    ($t:ty) => {
        impl RedbRlp for $t {
            fn encode(&self, out: &mut dyn alloy_rlp::BufMut) { Encodable::encode(self, out); }
            fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> { Decodable::decode(buf) }
            fn rlp_length(&self) -> usize { Encodable::length(self) }
        }
    };
}

impl RedbRlp for u64 {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) { out.put_u64(*self); }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort);
        }
        if buf.len() == 8 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[..8]);
            *buf = &buf[8..];
            Ok(u64::from_be_bytes(b))
        } else {
            // Fallback to RLP for backward compatibility with existing data
            Decodable::decode(buf)
        }
    }
    fn rlp_length(&self) -> usize { 8 }
}
impl_redb_rlp!(Header);
impl_redb_rlp!(B256);
impl_redb_rlp!(Address);
impl_redb_rlp!(U256);
impl_redb_rlp!(Transaction);
impl_redb_rlp!(Receipt);
impl_redb_rlp!(BlockBody<Transaction>);
impl_redb_rlp!(Block<Transaction>);
impl_redb_rlp!(TrieAccount);
impl RedbRlp for Bytes {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_slice(&self.0);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let res = Bytes::from(buf.to_vec());
        *buf = &buf[buf.len()..];
        Ok(res)
    }
    fn rlp_length(&self) -> usize {
        self.len()
    }
}

impl RedbRlp for String {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) { Encodable::encode(self, out); }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        Decodable::decode(buf)
    }
    fn rlp_length(&self) -> usize { Encodable::length(self) }
}

impl RedbRlp for PayloadId {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) { Encodable::encode(&self.0, out); }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> { Ok(PayloadId::new(Decodable::decode(buf)?)) }
    fn rlp_length(&self) -> usize { Encodable::length(&self.0) }
}

impl RedbRlp for (Address, B256) {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let payload_length = Encodable::length(&self.0) + Encodable::length(&self.1);
        alloy_rlp::Header { list: true, payload_length }.encode(out);
        Encodable::encode(&self.0, out);
        Encodable::encode(&self.1, out);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let _h = alloy_rlp::Header::decode(buf)?;
        Ok((Decodable::decode(buf)?, Decodable::decode(buf)?))
    }
    fn rlp_length(&self) -> usize {
        let l = Encodable::length(&self.0) + Encodable::length(&self.1);
        alloy_rlp::length_of_length(l) + l
    }
}

impl RedbRlp for Vec<(Address, Option<TrieAccount>)> {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let mut list_buf = Vec::new();
        for (a, b) in self {
            let mut item_buf = Vec::new();
            Encodable::encode(a, &mut item_buf);
            match b {
                Some(acc) => Encodable::encode(acc, &mut item_buf),
                None => item_buf.push(alloy_rlp::EMPTY_STRING_CODE), // Represent None as empty string in RLP
            }
            
            let item_header = alloy_rlp::Header { list: true, payload_length: item_buf.len() };
            item_header.encode(&mut list_buf);
            list_buf.extend_from_slice(&item_buf);
        }
        let list_header = alloy_rlp::Header { list: true, payload_length: list_buf.len() };
        list_header.encode(out);
        out.put_slice(&list_buf);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let h = alloy_rlp::Header::decode(buf)?;
        let mut res = Vec::new();
        let (mut remaining, rest) = buf.split_at(h.payload_length);
        *buf = rest;
        while !remaining.is_empty() {
            let _item_h = alloy_rlp::Header::decode(&mut remaining)?;
            let addr: Address = Decodable::decode(&mut remaining)?;
            let acc = if remaining.first() == Some(&alloy_rlp::EMPTY_STRING_CODE) {
                remaining = &remaining[1..];
                None
            } else {
                Some(Decodable::decode(&mut remaining)?)
            };
            res.push((addr, acc));
        }
        Ok(res)
    }
    fn rlp_length(&self) -> usize {
        let mut payload_length = 0;
        for (a, b) in self {
            let item_len = Encodable::length(a) + match b {
                Some(acc) => Encodable::length(acc),
                None => 1,
            };
            payload_length += alloy_rlp::length_of_length(item_len) + item_len;
        }
        alloy_rlp::length_of_length(payload_length) + payload_length
    }
}

impl RedbRlp for Vec<((Address, B256), Option<U256>)> {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let mut list_buf = Vec::new();
        for (k, v) in self {
            let mut item_buf = Vec::new();
            // Key (Address, B256)
            let mut key_buf = Vec::new();
            Encodable::encode(&k.0, &mut key_buf);
            Encodable::encode(&k.1, &mut key_buf);
            let key_header = alloy_rlp::Header { list: true, payload_length: key_buf.len() };
            key_header.encode(&mut item_buf);
            item_buf.extend_from_slice(&key_buf);

            // Value Option<U256>
            match v {
                Some(val) => Encodable::encode(val, &mut item_buf),
                None => item_buf.push(alloy_rlp::EMPTY_STRING_CODE),
            }

            let item_header = alloy_rlp::Header { list: true, payload_length: item_buf.len() };
            item_header.encode(&mut list_buf);
            list_buf.extend_from_slice(&item_buf);
        }
        let list_header = alloy_rlp::Header { list: true, payload_length: list_buf.len() };
        list_header.encode(out);
        out.put_slice(&list_buf);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let h = alloy_rlp::Header::decode(buf)?;
        let mut res = Vec::new();
        let (mut remaining, rest) = buf.split_at(h.payload_length);
        *buf = rest;
        while !remaining.is_empty() {
            let _item_h = alloy_rlp::Header::decode(&mut remaining)?;
            
            // Decode key
            let _key_h = alloy_rlp::Header::decode(&mut remaining)?;
            let addr: Address = Decodable::decode(&mut remaining)?;
            let key: B256 = Decodable::decode(&mut remaining)?;

            // Decode value
            let val = if remaining.first() == Some(&alloy_rlp::EMPTY_STRING_CODE) {
                remaining = &remaining[1..];
                None
            } else {
                Some(Decodable::decode(&mut remaining)?)
            };

            res.push(((addr, key), val));
        }
        Ok(res)
    }
    fn rlp_length(&self) -> usize {
        let mut payload_length = 0;
        for (k, v) in self {
            let key_len = Encodable::length(&k.0) + Encodable::length(&k.1);
            let item_len = (alloy_rlp::length_of_length(key_len) + key_len) + match v {
                Some(val) => Encodable::length(val),
                None => 1,
            };
            payload_length += alloy_rlp::length_of_length(item_len) + item_len;
        }
        alloy_rlp::length_of_length(payload_length) + payload_length
    }
}

impl RedbRlp for Vec<(Address, B256, U256)> {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let mut payload_length = 0;
        for (a, b, c) in self {
            let item_len = Encodable::length(a) + Encodable::length(b) + Encodable::length(c);
            payload_length += alloy_rlp::length_of_length(item_len) + item_len;
        }
        alloy_rlp::Header { list: true, payload_length }.encode(out);
        for (a, b, c) in self {
            let item_len = Encodable::length(a) + Encodable::length(b) + Encodable::length(c);
            alloy_rlp::Header { list: true, payload_length: item_len }.encode(out);
            Encodable::encode(a, out);
            Encodable::encode(b, out);
            Encodable::encode(c, out);
        }
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let h = alloy_rlp::Header::decode(buf)?;
        let mut res = Vec::new();
        let mut remaining = &buf[..h.payload_length];
        *buf = &buf[h.payload_length..];
        while !remaining.is_empty() {
            let _item_h = alloy_rlp::Header::decode(&mut remaining)?;
            res.push((Decodable::decode(&mut remaining)?, Decodable::decode(&mut remaining)?, Decodable::decode(&mut remaining)?));
        }
        Ok(res)
    }
    fn rlp_length(&self) -> usize {
        let mut payload_length = 0;
        for (a, b, c) in self {
            let item_len = Encodable::length(a) + Encodable::length(b) + Encodable::length(c);
            payload_length += alloy_rlp::length_of_length(item_len) + item_len;
        }
        alloy_rlp::length_of_length(payload_length) + payload_length
    }
}

impl RedbRlp for (Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1) {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let mut list_buf = Vec::new();
        Encodable::encode(&self.0, &mut list_buf);
        Encodable::encode(&self.1, &mut list_buf);
        Encodable::encode(&self.2, &mut list_buf);
        
        // Bundle
        let mut bundle_buf = Vec::new();
        Encodable::encode(&self.3.commitments, &mut bundle_buf);
        Encodable::encode(&self.3.proofs, &mut bundle_buf);
        Encodable::encode(&self.3.blobs, &mut bundle_buf);
        
        let bundle_header = alloy_rlp::Header { list: true, payload_length: bundle_buf.len() };
        bundle_header.encode(&mut list_buf);
        list_buf.extend_from_slice(&bundle_buf);

        let list_header = alloy_rlp::Header { list: true, payload_length: list_buf.len() };
        list_header.encode(out);
        out.put_slice(&list_buf);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let h = alloy_rlp::Header::decode(buf)?;
        if !h.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let (mut body, rest) = buf.split_at(h.payload_length);
        *buf = rest;

        let block = Decodable::decode(&mut body)?;
        let receipts = Decodable::decode(&mut body)?;
        let metas = Decodable::decode(&mut body)?;
        
        let h_bundle = alloy_rlp::Header::decode(&mut body)?;
        if !h_bundle.list {
             return Err(alloy_rlp::Error::UnexpectedString);
        }
        let (mut bundle_body, bundle_rest) = body.split_at(h_bundle.payload_length);
        body = bundle_rest;

        let commitments = Decodable::decode(&mut bundle_body)?;
        let proofs = Decodable::decode(&mut bundle_body)?;
        let blobs = Decodable::decode(&mut bundle_body)?;

        if !bundle_body.is_empty() {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        let bundle = BlobsBundleV1 {
            commitments,
            proofs,
            blobs,
        };
        
        if !body.is_empty() {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok((block, receipts, metas, bundle))
    }
    fn rlp_length(&self) -> usize {
        let mut l = Encodable::length(&self.0) +
                Encodable::length(&self.1) +
                Encodable::length(&self.2);
        
        let bundle_payload_len = Encodable::length(&self.3.commitments) +
                                Encodable::length(&self.3.proofs) +
                                Encodable::length(&self.3.blobs);
        
        l += alloy_rlp::length_of_length(bundle_payload_len) + bundle_payload_len;
        alloy_rlp::length_of_length(l) + l
    }
}

impl RedbRlp for (B256, u64) {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let payload_length = Encodable::length(&self.0) + Encodable::length(&self.1);
        alloy_rlp::Header { list: true, payload_length }.encode(out);
        Encodable::encode(&self.0, out);
        Encodable::encode(&self.1, out);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let _h = alloy_rlp::Header::decode(buf)?;
        Ok((Decodable::decode(buf)?, Decodable::decode(buf)?))
    }
    fn rlp_length(&self) -> usize {
        let l = Encodable::length(&self.0) + Encodable::length(&self.1);
        alloy_rlp::length_of_length(l) + l
    }
}

impl<T: RedbRlp> Value for RlpValue<T> {
    type SelfType<'a> = T;
    type AsBytes<'a> = Vec<u8>;
    fn fixed_width() -> Option<usize> { None }
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a> where Self: 'a {
        let mut d = data;
        match T::decode(&mut d) {
            Ok(v) => v,
            Err(e) => panic!("Failed to decode {}: {:?}", std::any::type_name::<T>(), e),
        }
    }
    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a> where Self: 'a, Self: 'b {
        let mut buf = Vec::with_capacity(value.rlp_length());
        value.encode(&mut buf);
        buf
    }
    fn type_name() -> redb::TypeName { redb::TypeName::new(std::any::type_name::<T>()) }
}

impl<T: RedbRlp> Key for RlpValue<T> {
    fn compare(data1: &[u8], data2: &[u8]) -> std::cmp::Ordering { data1.cmp(data2) }
}
impl RedbRlp for PeerEntry {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let payload_length = Encodable::length(&self.peer_id) + Encodable::length(&self.discovery_addr.to_string()) + Encodable::length(&self.p2p_addr.to_string());
        alloy_rlp::Header { list: true, payload_length }.encode(out);
        Encodable::encode(&self.peer_id, out);
        Encodable::encode(&self.discovery_addr.to_string(), out);
        Encodable::encode(&self.p2p_addr.to_string(), out);
    }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let h = alloy_rlp::Header::decode(buf)?;
        if !h.list {
             return Err(alloy_rlp::Error::Custom("Expected list for PeerEntry"));
        }
        let peer_id: String = Decodable::decode(buf)?;
        let disc_addr_str: String = Decodable::decode(buf)?;
        let p2p_addr_str: String = Decodable::decode(buf)?;
        
        Ok(PeerEntry {
            peer_id,
            discovery_addr: disc_addr_str.parse().map_err(|_| alloy_rlp::Error::Custom("Invalid discovery_addr"))?,
            p2p_addr: p2p_addr_str.parse().map_err(|_| alloy_rlp::Error::Custom("Invalid p2p_addr"))?,
        })
    }
    fn rlp_length(&self) -> usize {
        let l = Encodable::length(&self.peer_id) + Encodable::length(&self.discovery_addr.to_string()) + Encodable::length(&self.p2p_addr.to_string());
        alloy_rlp::length_of_length(l) + l
    }
}

impl RedbRlp for ReceiptMeta {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) { alloy_rlp::Encodable::encode(self, out); }
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> { alloy_rlp::Decodable::decode(buf) }
    fn rlp_length(&self) -> usize { alloy_rlp::Encodable::length(self) }
}

