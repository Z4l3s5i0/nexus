use alloy_trie::nodes::RlpNode as ChildNode;
use alloy_trie::{Nibbles, EMPTY_ROOT_HASH};
use alloy_rlp::{Encodable, Decodable, Header, EMPTY_STRING_CODE};
use std::collections::HashMap;
use alloy_primitives::{B256, keccak256, Bytes, Address};
use crate::read_traits::StateProvider;
use crate::write_traits::StateWriter;

fn encode_path(nibbles: &Nibbles, is_leaf: bool) -> Vec<u8> {
    let mut res = Vec::with_capacity(nibbles.len() / 2 + 1);
    let mut flag = if is_leaf { 0x20 } else { 0x00 };
    if nibbles.len() % 2 != 0 {
        flag |= 0x10;
        flag |= nibbles.get(0).unwrap();
        res.push(flag);
        for i in (1..nibbles.len()).step_by(2) {
            res.push((nibbles.get(i).unwrap() << 4) | nibbles.get(i+1).unwrap());
        }
    } else {
        res.push(flag);
        for i in (0..nibbles.len()).step_by(2) {
            res.push((nibbles.get(i).unwrap() << 4) | nibbles.get(i+1).unwrap());
        }
    }
    res
}

fn decode_path(data: &[u8]) -> Option<(Nibbles, bool)> {
    if data.is_empty() { return None; }
    let first = data[0];
    let is_leaf = (first & 0x20) != 0;
    let is_odd = (first & 0x10) != 0;
    let mut nibbles = Vec::new();
    if is_odd {
        nibbles.push(first & 0x0F);
    }
    for &b in &data[1..] {
        nibbles.push(b >> 4);
        nibbles.push(b & 0x0F);
    }
    Some((Nibbles::from_nibbles(nibbles), is_leaf))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyLeafNode {
    pub key: Nibbles,
    pub value: Vec<u8>,
}

impl MyLeafNode {
    pub fn new(key: Nibbles, value: Vec<u8>) -> Self {
        Self { key, value }
    }
}

impl Encodable for MyLeafNode {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let encoded_key = encode_path(&self.key, true);
        let payload_length = Encodable::length(&encoded_key.as_slice()) + Encodable::length(&self.value.as_slice());
        Header { list: true, payload_length }.encode(out);
        encoded_key.as_slice().encode(out);
        self.value.as_slice().encode(out);
    }

    fn length(&self) -> usize {
        let encoded_key = encode_path(&self.key, true);
        let payload_length = Encodable::length(&encoded_key.as_slice()) + Encodable::length(&self.value.as_slice());
        Header { list: true, payload_length }.length() + payload_length
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyExtensionNode {
    pub key: Nibbles,
    pub child: ChildNode,
}

impl MyExtensionNode {
    pub fn new(key: Nibbles, child: ChildNode) -> Self {
        Self { key, child }
    }
}

impl Encodable for MyExtensionNode {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let encoded_key = encode_path(&self.key, false);
        let payload_length = Encodable::length(&encoded_key.as_slice()) + self.child.len();
        Header { list: true, payload_length }.encode(out);
        encoded_key.as_slice().encode(out);
        out.put_slice(self.child.as_slice());
    }

    fn length(&self) -> usize {
        let encoded_key = encode_path(&self.key, false);
        let payload_length = Encodable::length(&encoded_key.as_slice()) + self.child.len();
        Header { list: true, payload_length }.length() + payload_length
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MyBranchNode {
    pub stack: [ChildNode; 16],
    pub value: Option<Vec<u8>>,
}

impl Encodable for MyBranchNode {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let mut payload_length = 0;
        for child in &self.stack {
            if child.is_empty() {
                payload_length += 1;
            } else {
                payload_length += child.len();
            }
        }
        payload_length += self.value.as_ref().map(|v| Encodable::length(&v.as_slice())).unwrap_or(1);

        let list_header = Header {
            list: true,
            payload_length,
        };
        
        list_header.encode(out);
        for child in &self.stack {
            if child.is_empty() {
                out.put_u8(EMPTY_STRING_CODE);
            } else {
                out.put_slice(child.as_slice());
            }
        }
        if let Some(v) = &self.value {
            v.as_slice().encode(out);
        } else {
            out.put_u8(EMPTY_STRING_CODE);
        }
    }

    fn length(&self) -> usize {
        let mut payload_length = 0;
        for child in &self.stack {
            if child.is_empty() {
                payload_length += 1;
            } else {
                payload_length += child.len();
            }
        }
        payload_length += self.value.as_ref().map(|v| Encodable::length(&v.as_slice())).unwrap_or(1);
        Header { list: true, payload_length }.length() + payload_length
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MyTrieNode {
    EmptyRoot,
    Leaf(MyLeafNode),
    Extension(MyExtensionNode),
    Branch(MyBranchNode),
}

impl MyTrieNode {
    pub fn decode(buf: &mut &[u8]) -> anyhow::Result<Self> {
        if buf.is_empty() {
             return Err(anyhow::anyhow!("Empty RLP buffer"));
        }
        
        let mut b = *buf;
        let header = Header::decode(&mut b).map_err(|e| anyhow::anyhow!("RLP header decode error: {}", e))?;
        if !header.list {
            if header.payload_length == 0 {
                *buf = b;
                return Ok(MyTrieNode::EmptyRoot);
            }
            return Err(anyhow::anyhow!("Expected RLP list for trie node, got string with length {}", header.payload_length));
        }

        let payload = &b[..header.payload_length];
        let next_buf = &b[header.payload_length..];

        let mut items = Vec::new();
        let mut temp_payload = payload;
        while !temp_payload.is_empty() {
            let item_start = temp_payload;
            let item_header = Header::decode(&mut temp_payload).map_err(|e| anyhow::anyhow!("RLP item header decode error: {}", e))?;
            let header_len = item_start.len() - temp_payload.len();
            let item_len = header_len + item_header.payload_length;
            if item_len > item_start.len() {
                return Err(anyhow::anyhow!("RLP item length {} exceeds remaining payload {}", item_len, item_start.len()));
            }
            items.push(&item_start[..item_len]);
            temp_payload = &item_start[item_len..];
        }

        if items.len() == 2 {
            let mut item0 = items[0];
            let h0 = Header::decode(&mut item0).map_err(|e| anyhow::anyhow!("Key header decode error: {}", e))?;
            if h0.list {
                return Err(anyhow::anyhow!("Key must be a string"));
            }
            let (nibbles, is_leaf) = decode_path(item0).ok_or_else(|| anyhow::anyhow!("Invalid hex-prefix encoding"))?;
            
            *buf = next_buf;
            if is_leaf {
                let mut s = items[1];
                let value = <Bytes>::decode(&mut s).map_err(|e| anyhow::anyhow!("Leaf value decode error: {}", e))?;
                Ok(MyTrieNode::Leaf(MyLeafNode::new(nibbles, value.to_vec())))
            } else {
                let child = ChildNode::from_raw_rlp(items[1]).map_err(|e| anyhow::anyhow!("Extension child decode error: {}", e))?;
                Ok(MyTrieNode::Extension(MyExtensionNode::new(nibbles, child)))
            }
        } else if items.len() == 17 {
            *buf = next_buf;
            let mut branch = MyBranchNode::default();
            for (idx, item) in items.into_iter().enumerate() {
                if idx == 16 {
                    if item != [EMPTY_STRING_CODE] {
                        let mut s = item;
                        let value = <Bytes>::decode(&mut s).map_err(|e| anyhow::anyhow!("Branch value decode error: {}", e))?;
                        branch.value = Some(value.to_vec());
                    }
                } else if item != [EMPTY_STRING_CODE] {
                    branch.stack[idx] = ChildNode::from_raw_rlp(item).map_err(|e| anyhow::anyhow!("Branch child decode error: {}", e))?;
                }
            }
            Ok(MyTrieNode::Branch(branch))
        } else {
             Err(anyhow::anyhow!("Invalid number of items in trie node RLP list: {}", items.len()))
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            MyTrieNode::EmptyRoot => {
                out.push(EMPTY_STRING_CODE);
            }
            MyTrieNode::Leaf(leaf) => {
                leaf.encode(out);
            }
            MyTrieNode::Extension(ext) => {
                ext.encode(out);
            }
            MyTrieNode::Branch(branch) => {
                branch.encode(out);
            }
        }
    }
}

#[derive(Default)]
pub struct MemoryState {
    pub nodes: HashMap<B256, Bytes>,
}

impl StateProvider for MemoryState {
    fn trie_node(&self, hash: B256) -> anyhow::Result<Option<Bytes>> {
        Ok(self.nodes.get(&hash).cloned())
    }
    // Implement other methods as needed, or just panic if not used
    fn plain_state(&self, _address: Address) -> anyhow::Result<Option<Bytes>> { unreachable!() }
    fn hashed_state(&self, _hash: B256) -> anyhow::Result<Option<Bytes>> { unreachable!() }
}

impl StateWriter for MemoryState {
    fn update_trie_node(&self, _hash: B256, _node: Bytes) -> anyhow::Result<()> {
        // MemoryState is usually immutable for provider, but EthTrie needs &S
        // We can use RefCell if needed, but EthTrie already has dirty map.
        Ok(())
    }
    fn update_plain_state(&self, _address: Address, _state: Bytes) -> anyhow::Result<()> { unreachable!() }
    fn remove_plain_state(&self, _address: Address) -> anyhow::Result<()> { unreachable!() }
    fn update_hashed_state(&self, _hash: B256, _state: Bytes) -> anyhow::Result<()> { unreachable!() }
}

pub fn calculate_trie_root(leaves: Vec<(B256, Vec<u8>)>) -> anyhow::Result<B256> {
    let state = MemoryState::default();
    let mut trie = EthTrie::new(&state, EMPTY_ROOT_HASH);
    for (key, value) in leaves {
        trie.insert(key, value)?;
    }
    Ok(trie.root_hash())
}

pub struct EthTrie<'a, S: StateProvider + StateWriter> {
    state: &'a S,
    root_hash: B256,
    cache: HashMap<B256, MyTrieNode>,
    dirty: HashMap<B256, Vec<u8>>,
}

impl<'a, S: StateProvider + StateWriter> EthTrie<'a, S> {
    pub fn new(state: &'a S, root_hash: B256) -> Self {
        Self {
            state,
            root_hash,
            cache: HashMap::new(),
            dirty: HashMap::new(),
        }
    }

    pub fn root_hash(&self) -> B256 {
        self.root_hash
    }

    pub fn get_nibbles(&mut self, nibbles: Nibbles) -> anyhow::Result<Option<Vec<u8>>> {
        self.get_recursive(self.root_hash, nibbles)
    }

    pub fn insert_nibbles(&mut self, nibbles: Nibbles, value: Vec<u8>) -> anyhow::Result<()> {
        let root = self.root_hash;
        self.root_hash = self.insert_recursive(root, nibbles, value)?;
        Ok(())
    }

    pub fn delete_nibbles(&mut self, nibbles: Nibbles) -> anyhow::Result<()> {
        let root = self.root_hash;
        self.root_hash = self.delete_recursive(root, nibbles)?;
        Ok(())
    }

    pub fn get(&mut self, key: B256) -> anyhow::Result<Option<Vec<u8>>> {
        let nibbles = Nibbles::unpack(key);
        self.get_nibbles(nibbles)
    }

    fn get_recursive(&mut self, node_hash: B256, nibbles: Nibbles) -> anyhow::Result<Option<Vec<u8>>> {
        if node_hash == EMPTY_ROOT_HASH {
            return Ok(None);
        }

        let node = self.get_node(node_hash)?;
        self.get_recursive_node(node, nibbles)
    }

    fn get_recursive_node(&mut self, node: MyTrieNode, nibbles: Nibbles) -> anyhow::Result<Option<Vec<u8>>> {
        match node {
            MyTrieNode::EmptyRoot => Ok(None),
            MyTrieNode::Leaf(leaf) => {
                if leaf.key == nibbles {
                    Ok(Some(leaf.value))
                } else {
                    Ok(None)
                }
            }
            MyTrieNode::Extension(ext) => {
                if nibbles.starts_with(&ext.key) {
                    let remaining = if ext.key.len() < nibbles.len() { nibbles.slice(ext.key.len()..) } else { Nibbles::default() };
                    self.get_recursive_rlp(&ext.child, remaining)
                } else {
                    Ok(None)
                }
            }
            MyTrieNode::Branch(branch) => {
                if nibbles.is_empty() {
                    return Ok(branch.value);
                }
                let index = nibbles.get(0).ok_or_else(|| anyhow::anyhow!("Nibbles empty"))? as usize;
                let remaining = if 1 < nibbles.len() { nibbles.slice(1..) } else { Nibbles::default() };
                self.get_recursive_rlp(&branch.stack[index], remaining)
            }
        }
    }

    fn get_recursive_rlp(&mut self, rlp_node: &ChildNode, nibbles: Nibbles) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(hash) = rlp_node.as_hash() {
            self.get_recursive(hash, nibbles)
        } else {
            let mut data = rlp_node.as_slice();
            if data.is_empty() {
                return Ok(None);
            }
            let node = MyTrieNode::decode(&mut data)?;
            self.get_recursive_node(node, nibbles)
        }
    }

    pub fn insert(&mut self, key: B256, value: Vec<u8>) -> anyhow::Result<()> {
        let nibbles = Nibbles::unpack(key);
        self.insert_nibbles(nibbles, value)
    }

    fn insert_recursive(&mut self, node_hash: B256, nibbles: Nibbles, value: Vec<u8>) -> anyhow::Result<B256> {
        if node_hash == EMPTY_ROOT_HASH {
            let leaf = MyLeafNode::new(nibbles, value);
            return Ok(self.store_node(MyTrieNode::Leaf(leaf))?);
        }

        let node = self.get_node(node_hash)?;
        if let MyTrieNode::EmptyRoot = node {
             let leaf = MyLeafNode::new(nibbles, value);
             return Ok(self.store_node(MyTrieNode::Leaf(leaf))?);
        }
        let new_node = match node {
            MyTrieNode::EmptyRoot => {
                MyTrieNode::Leaf(MyLeafNode::new(nibbles, value))
            }
            MyTrieNode::Leaf(mut leaf) => {
                let common = nibbles.common_prefix_length(&leaf.key);
                if common == nibbles.len() && common == leaf.key.len() {
                    // Update existing leaf
                    leaf.value = value;
                    MyTrieNode::Leaf(leaf)
                } else {
                    // Split
                    let mut branch = MyBranchNode::default();
                    
                    if common == leaf.key.len() {
                        // Current leaf is prefix of new key
                        branch.value = Some(leaf.value);
                        let remaining_new = nibbles.slice(common..);
                        let index_new = remaining_new.get(0).expect("common < nibbles.len()") as usize;
                        let child_new_key = if 1 < remaining_new.len() { remaining_new.slice(1..) } else { Nibbles::default() };
                        let child_new = MyLeafNode::new(child_new_key, value);
                        branch.stack[index_new] = self.store_node_to_rlp(MyTrieNode::Leaf(child_new))?;
                    } else if common == nibbles.len() {
                        // New key is prefix of current leaf
                        branch.value = Some(value);
                        let remaining_leaf = leaf.key.slice(common..);
                        let index_leaf = remaining_leaf.get(0).expect("common < leaf.key.len()") as usize;
                        let child_leaf_key = if 1 < remaining_leaf.len() { remaining_leaf.slice(1..) } else { Nibbles::default() };
                        let child_leaf = MyLeafNode::new(child_leaf_key, leaf.value);
                        branch.stack[index_leaf] = self.store_node_to_rlp(MyTrieNode::Leaf(child_leaf))?;
                    } else {
                        // Diverge
                        let index_leaf = leaf.key.get(common).expect("common < leaf.key.len()") as usize;
                        let index_new = nibbles.get(common).expect("common < nibbles.len()") as usize;
                        
                        let remaining_leaf = if common + 1 < leaf.key.len() { leaf.key.slice(common + 1..) } else { Nibbles::default() };
                        let child_leaf = MyLeafNode::new(remaining_leaf, leaf.value);
                        branch.stack[index_leaf] = self.store_node_to_rlp(MyTrieNode::Leaf(child_leaf))?;
                        
                        let remaining_new = if common + 1 < nibbles.len() { nibbles.slice(common + 1..) } else { Nibbles::default() };
                        let child_new = MyLeafNode::new(remaining_new, value);
                        branch.stack[index_new] = self.store_node_to_rlp(MyTrieNode::Leaf(child_new))?;
                    }
                    
                    if common > 0 {
                        let ext = MyExtensionNode::new(nibbles.slice(..common), self.store_node_to_rlp(MyTrieNode::Branch(branch))?);
                        MyTrieNode::Extension(ext)
                    } else {
                        MyTrieNode::Branch(branch)
                    }
                }
            }
            MyTrieNode::Extension(mut ext) => {
                let common = nibbles.common_prefix_length(&ext.key);

                if common == ext.key.len() {
                        // Full match of extension, recurse to child
                        let remaining = nibbles.slice(common..);
                        let new_child_rlp = self.insert_recursive_rlp(ext.child, remaining, value)?;
                        ext.child = new_child_rlp;
                        MyTrieNode::Extension(ext)
                    } else {
                        // Split extension
                        let mut branch = MyBranchNode::default();
                        
                        if common == nibbles.len() {
                            // New key is a prefix of extension key
                            branch.value = Some(value);
                            let remaining_ext = ext.key.slice(common..);
                            let index_ext = remaining_ext.get(0).expect("common < ext.key.len()") as usize;
                            let sub_ext_key = if 1 < remaining_ext.len() { remaining_ext.slice(1..) } else { Nibbles::default() };
                            if sub_ext_key.is_empty() {
                                branch.stack[index_ext] = ext.child;
                            } else {
                                let sub_ext = MyExtensionNode::new(sub_ext_key, ext.child);
                                branch.stack[index_ext] = self.store_node_to_rlp(MyTrieNode::Extension(sub_ext))?;
                            }
                        } else {
                            // Diverge
                            let index_ext = ext.key.get(common).expect("common < ext.key.len()") as usize;
                            let index_new = nibbles.get(common).expect("common < nibbles.len()") as usize;

                            let remaining_ext = if common + 1 < ext.key.len() { ext.key.slice(common + 1..) } else { Nibbles::default() };
                            if remaining_ext.is_empty() {
                                 branch.stack[index_ext] = ext.child;
                            } else {
                                 let sub_ext = MyExtensionNode::new(remaining_ext, ext.child);
                                 branch.stack[index_ext] = self.store_node_to_rlp(MyTrieNode::Extension(sub_ext))?;
                            }
                            
                            let remaining_new = if common + 1 < nibbles.len() { nibbles.slice(common + 1..) } else { Nibbles::default() };
                            let child_new = MyLeafNode::new(remaining_new, value);
                            branch.stack[index_new] = self.store_node_to_rlp(MyTrieNode::Leaf(child_new))?;
                        }
                        
                        if common > 0 {
                             let new_ext = MyExtensionNode::new(ext.key.slice(..common), self.store_node_to_rlp(MyTrieNode::Branch(branch))?);
                             MyTrieNode::Extension(new_ext)
                        } else {
                             MyTrieNode::Branch(branch)
                        }
                    }
            }
            MyTrieNode::Branch(mut branch) => {
                if nibbles.is_empty() {
                    branch.value = Some(value);
                    MyTrieNode::Branch(branch)
                } else {
                    let index = nibbles.get(0).ok_or_else(|| anyhow::anyhow!("Index error"))? as usize;
                    if index >= 16 {
                        return Err(anyhow::anyhow!("Branch index out of bounds: {}", index));
                    }
                    let remaining = if 1 < nibbles.len() { nibbles.slice(1..) } else { Nibbles::default() };
                    let new_child_rlp = self.insert_recursive_rlp(branch.stack[index].clone(), remaining, value)?;
                    branch.stack[index] = new_child_rlp;
                    MyTrieNode::Branch(branch)
                }
            }
        };

        self.store_node(new_node)
    }

    fn insert_recursive_rlp(&mut self, rlp_node: ChildNode, nibbles: Nibbles, value: Vec<u8>) -> anyhow::Result<ChildNode> {
        if let Some(hash) = rlp_node.as_hash() {
            let new_hash = self.insert_recursive(hash, nibbles, value)?;
            Ok(ChildNode::word_rlp(&new_hash))
        } else {
            let mut data = rlp_node.as_slice();
            let node = if data.is_empty() {
                MyTrieNode::EmptyRoot
            } else {
                MyTrieNode::decode(&mut data)?
            };
            let dummy_hash = self.store_node(node)?;
            let new_hash = self.insert_recursive(dummy_hash, nibbles, value)?;
            let new_node = self.get_node(new_hash)?;
            Ok(self.store_node_to_rlp(new_node)?)
        }
    }

    fn store_node(&mut self, node: MyTrieNode) -> anyhow::Result<B256> {
        let mut buf = Vec::new();
        node.encode(&mut buf);
        let hash = keccak256(&buf);
        self.dirty.insert(hash, buf);
        self.cache.insert(hash, node);
        Ok(hash)
    }

    fn store_node_to_rlp(&mut self, node: MyTrieNode) -> anyhow::Result<ChildNode> {
        let mut buf = Vec::new();
        node.encode(&mut buf);
        if buf.len() >= 32 {
            let hash = keccak256(&buf);
            self.dirty.insert(hash, buf);
            self.cache.insert(hash, node);
            Ok(ChildNode::word_rlp(&hash))
        } else {
            Ok(ChildNode::from_rlp(&buf))
        }
    }

    fn get_node(&mut self, hash: B256) -> anyhow::Result<MyTrieNode> {
        if hash == EMPTY_ROOT_HASH {
            return Ok(MyTrieNode::EmptyRoot);
        }
        if let Some(node) = self.cache.get(&hash) {
            return Ok(node.clone());
        }

        let data = if let Some(dirty_data) = self.dirty.get(&hash) {
            dirty_data.clone()
        } else {
            match self.state.trie_node(hash)? {
                Some(bytes) => bytes.to_vec(),
                None => {
                    return Err(anyhow::anyhow!("Trie node not found: {:?}", hash));
                }
            }
        };

        let mut slice = &data[..];
        let node = MyTrieNode::decode(&mut slice)?;
        self.cache.insert(hash, node.clone());
        Ok(node)
    }

    pub fn delete(&mut self, key: B256) -> anyhow::Result<()> {
        let nibbles = Nibbles::unpack(key);
        self.delete_nibbles(nibbles)
    }

    fn delete_recursive(&mut self, node_hash: B256, nibbles: Nibbles) -> anyhow::Result<B256> {
        // debug!("[Trie] delete_recursive: hash={:?}, nibbles={:?}", node_hash, nibbles);
        if node_hash == EMPTY_ROOT_HASH {
            return Ok(EMPTY_ROOT_HASH);
        }

        let node = self.get_node(node_hash)?;
        let new_node = self.delete_recursive_node(node, nibbles)?;

        if matches!(new_node, MyTrieNode::EmptyRoot) {
            Ok(EMPTY_ROOT_HASH)
        } else {
            self.store_node(new_node)
        }
    }

    fn delete_recursive_node(&mut self, node: MyTrieNode, nibbles: Nibbles) -> anyhow::Result<MyTrieNode> {
        match node {
            MyTrieNode::EmptyRoot => Ok(MyTrieNode::EmptyRoot),
            MyTrieNode::Leaf(leaf) => {
                if leaf.key == nibbles {
                    Ok(MyTrieNode::EmptyRoot)
                } else {
                    Ok(MyTrieNode::Leaf(leaf))
                }
            }
            MyTrieNode::Extension(mut ext) => {
                if nibbles.starts_with(&ext.key) {
                    let remaining = if ext.key.len() < nibbles.len() { nibbles.slice(ext.key.len()..) } else { Nibbles::default() };
                    let new_child_rlp = self.delete_recursive_rlp(ext.child, remaining)?;
                    if new_child_rlp.is_empty() {
                        Ok(MyTrieNode::EmptyRoot)
                    } else {
                        ext.child = new_child_rlp;
                        // Collapse extension
                        let child_node = self.get_node_from_rlp(&ext.child)?;
                        match child_node {
                            MyTrieNode::Extension(child_ext) => {
                                let mut new_key = ext.key.clone();
                                new_key.extend(&child_ext.key);
                                Ok(MyTrieNode::Extension(MyExtensionNode::new(new_key, child_ext.child)))
                            }
                            _ => Ok(MyTrieNode::Extension(ext)),
                        }
                    }
                } else {
                    Ok(MyTrieNode::Extension(ext))
                }
            }
            MyTrieNode::Branch(mut branch) => {
                if !nibbles.is_empty() {
                    let index = nibbles.get(0).ok_or_else(|| anyhow::anyhow!("Index error"))? as usize;
                    if index >= 16 {
                        return Err(anyhow::anyhow!("Branch index out of bounds: {}", index));
                    }
                    let remaining = if 1 < nibbles.len() { nibbles.slice(1..) } else { Nibbles::default() };
                    let new_child_rlp = self.delete_recursive_rlp(branch.stack[index].clone(), remaining)?;
                    branch.stack[index] = new_child_rlp;
                } else {
                    branch.value = None;
                }

                // Check if branch can be collapsed
                let children_count = branch.stack.iter().filter(|c| !c.is_empty()).count();
                let has_value = branch.value.is_some();

                if children_count == 0 && !has_value {
                    Ok(MyTrieNode::EmptyRoot)
                } else if children_count == 1 && !has_value {
                    let (index, child_rlp) = branch.stack.iter().enumerate().find(|(_, c)| !c.is_empty()).ok_or_else(|| anyhow::anyhow!("Child not found"))?;
                    let child_node = self.get_node_from_rlp(child_rlp)?;
                    match child_node {
                        MyTrieNode::Leaf(mut leaf) => {
                            let mut new_key = Nibbles::from_nibbles([index as u8]);
                            new_key.extend(&leaf.key);
                            leaf.key = new_key;
                            Ok(MyTrieNode::Leaf(leaf))
                        }
                        MyTrieNode::Extension(mut ext) => {
                            let mut new_key = Nibbles::from_nibbles([index as u8]);
                            new_key.extend(&ext.key);
                            ext.key = new_key;
                            Ok(MyTrieNode::Extension(ext))
                        }
                        MyTrieNode::Branch(_) => {
                            let new_key = Nibbles::from_nibbles([index as u8]);
                            Ok(MyTrieNode::Extension(MyExtensionNode::new(new_key, child_rlp.clone())))
                        }
                        _ => Ok(MyTrieNode::Branch(branch)),
                    }
                } else {
                    Ok(MyTrieNode::Branch(branch))
                }
            }
        }
    }

    fn delete_recursive_rlp(&mut self, rlp_node: ChildNode, nibbles: Nibbles) -> anyhow::Result<ChildNode> {
        if let Some(hash) = rlp_node.as_hash() {
            let new_hash = self.delete_recursive(hash, nibbles)?;
            if new_hash == EMPTY_ROOT_HASH {
                Ok(ChildNode::default())
            } else {
                Ok(ChildNode::word_rlp(&new_hash))
            }
        } else {
            let mut data = rlp_node.as_slice();
            if data.is_empty() {
                return Ok(ChildNode::default());
            }
            let node = MyTrieNode::decode(&mut data)?;
            let new_node = self.delete_recursive_node(node, nibbles)?;
            self.store_node_to_rlp(new_node)
        }
    }

    fn get_node_from_rlp(&mut self, rlp_node: &ChildNode) -> anyhow::Result<MyTrieNode> {
        if let Some(hash) = rlp_node.as_hash() {
            self.get_node(hash)
        } else {
            let mut data = rlp_node.as_slice();
            if data.is_empty() {
                Ok(MyTrieNode::EmptyRoot)
            } else {
                Ok(MyTrieNode::decode(&mut data)?)
            }
        }
    }

    pub fn all_entries(&mut self) -> anyhow::Result<Vec<(B256, Vec<u8>)>> {
        let mut entries = Vec::new();
        if self.root_hash == alloy_trie::EMPTY_ROOT_HASH {
            return Ok(entries);
        }
        self.walk_recursive(self.root_hash, Nibbles::default(), &mut entries)?;
        Ok(entries)
    }

    fn walk_recursive(&mut self, hash: B256, path: Nibbles, entries: &mut Vec<(B256, Vec<u8>)>) -> anyhow::Result<()> {
        let node = self.get_node(hash)?;
        self.walk_recursive_node(node, path, entries)
    }

    fn walk_recursive_rlp(&mut self, rlp_node: &ChildNode, path: Nibbles, entries: &mut Vec<(B256, Vec<u8>)>) -> anyhow::Result<()> {
        let node = self.get_node_from_rlp(rlp_node)?;
        self.walk_recursive_node(node, path, entries)
    }

    fn walk_recursive_node(&mut self, node: MyTrieNode, path: Nibbles, entries: &mut Vec<(B256, Vec<u8>)>) -> anyhow::Result<()> {
        match node {
            MyTrieNode::Leaf(leaf) => {
                let mut full_path = path;
                full_path.extend(&leaf.key);
                if full_path.len() == 64 {
                    let key = B256::from_slice(&full_path.pack());
                    entries.push((key, leaf.value));
                }
            }
            MyTrieNode::Extension(ext) => {
                let mut next_path = path;
                next_path.extend(&ext.key);
                self.walk_recursive_rlp(&ext.child, next_path, entries)?;
            }
            MyTrieNode::Branch(branch) => {
                for i in 0..64 {
                    let child_rlp = &branch.stack[i];
                    if !child_rlp.is_empty() {
                        let mut next_path = path.clone();
                        next_path.push(i as u8);
                        self.walk_recursive_rlp(child_rlp, next_path, entries)?;
                    }
                }
                if let Some(val) = branch.value {
                    if path.len() == 64 {
                        let key = B256::from_slice(&path.pack());
                        entries.push((key, val));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn commit(&mut self) -> anyhow::Result<()> {
        for (hash, data) in self.dirty.drain() {
            self.state.update_trie_node(hash, data.into())?;
        }
        Ok(())
    }
}
