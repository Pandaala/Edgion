//! Shared binary radix build, freeze, and lookup implementation.

use std::marker::PhantomData;

use super::error::IpRadixError;

pub(super) trait BitKey: Copy {
    const WIDTH: u8;

    fn bit_at(self, depth: u8) -> bool;
}

impl BitKey for u32 {
    const WIDTH: u8 = 32;

    #[inline]
    fn bit_at(self, depth: u8) -> bool {
        ((self >> (Self::WIDTH - 1 - depth)) & 1) != 0
    }
}

impl BitKey for u128 {
    const WIDTH: u8 = 128;

    #[inline]
    fn bit_at(self, depth: u8) -> bool {
        ((self >> (Self::WIDTH - 1 - depth)) & 1) != 0
    }
}

#[derive(Debug, Clone)]
struct BuildNode {
    left: Option<Box<BuildNode>>,
    right: Option<Box<BuildNode>>,
    /// Interned group-label ID at this node. 0 = no rule terminates here.
    label_id: u16,
    prefix_len: u8,
}

impl BuildNode {
    fn new() -> Self {
        Self {
            left: None,
            right: None,
            label_id: 0,
            prefix_len: 0,
        }
    }
}

#[derive(Debug)]
pub(super) struct BinaryRadixBuilder<K: BitKey> {
    root: BuildNode,
    rule_count: usize,
    key: PhantomData<K>,
}

impl<K: BitKey> BinaryRadixBuilder<K> {
    pub(super) fn new() -> Self {
        Self {
            root: BuildNode::new(),
            rule_count: 0,
            key: PhantomData,
        }
    }

    pub(super) fn insert(&mut self, key: K, prefix_len: u8, label_id: u16, cidr_str: &str) -> Result<(), IpRadixError> {
        debug_assert!(label_id != 0, "label_id 0 is reserved");
        debug_assert!(prefix_len <= K::WIDTH, "prefix length exceeds key width");
        Self::insert_node(&mut self.root, key, prefix_len, label_id, 0, cidr_str)?;
        self.rule_count += 1;
        Ok(())
    }

    fn insert_node(
        node: &mut BuildNode,
        key: K,
        prefix_len: u8,
        label_id: u16,
        current_bit: u8,
        cidr_str: &str,
    ) -> Result<(), IpRadixError> {
        if current_bit == prefix_len {
            if node.label_id != 0 && node.label_id != label_id {
                return Err(IpRadixError::DuplicateCidr {
                    cidr: cidr_str.to_string(),
                    existing_label: format!("#{}", node.label_id),
                    new_label: format!("#{label_id}"),
                });
            }
            node.label_id = label_id;
            node.prefix_len = prefix_len;
            return Ok(());
        }

        let child = if key.bit_at(current_bit) {
            node.right.get_or_insert_with(|| Box::new(BuildNode::new()))
        } else {
            node.left.get_or_insert_with(|| Box::new(BuildNode::new()))
        };
        Self::insert_node(child, key, prefix_len, label_id, current_bit + 1, cidr_str)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rule_count == 0
    }

    pub(super) fn freeze(self) -> Result<FrozenBinaryRadixTree<K>, IpRadixError> {
        if self.is_empty() {
            return Err(IpRadixError::EmptyTree);
        }

        let node_count = count_nodes(&self.root);
        if node_count > u32::MAX as usize {
            return Err(IpRadixError::TooManyNodes {
                count: node_count,
                max: u32::MAX as usize,
            });
        }

        let mut flat_builder = FlatTreeBuilder::with_capacity(node_count);
        flat_builder.flatten_node(&self.root)?;
        Ok(FrozenBinaryRadixTree {
            nodes: flat_builder.nodes,
            key: PhantomData,
        })
    }
}

impl<K: BitKey> Default for BinaryRadixBuilder<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// A flattened node shared by IPv4 and IPv6 trees.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FlatNode {
    /// Index of left child (0 if none).
    left_index: u32,
    /// Index of right child (0 if none).
    right_index: u32,
    /// Prefix length at this node (0 if not a terminal).
    prefix_len: u8,
    /// Interned group label ID. 0 = no rule terminates here.
    label_id: u16,
    _padding: [u8; 1],
}

#[derive(Debug, Clone)]
pub(super) struct FrozenBinaryRadixTree<K: BitKey> {
    nodes: Vec<FlatNode>,
    key: PhantomData<K>,
}

impl<K: BitKey> FrozenBinaryRadixTree<K> {
    #[inline]
    pub(super) fn match_key(&self, key: K) -> Option<u16> {
        let mut node_idx = 0;
        let mut last_match = None;
        let mut current_bit = 0;

        loop {
            let node = self.nodes.get(node_idx)?;
            if node.label_id != 0 {
                last_match = Some(node.label_id);
            }

            if current_bit >= K::WIDTH {
                break;
            }

            let next_idx = if key.bit_at(current_bit) {
                node.right_index
            } else {
                node.left_index
            };

            // The root occupies index 0, which is also the no-child sentinel. DFS
            // pre-order flattening guarantees that every real child index is non-zero.
            if next_idx == 0 {
                break;
            }

            node_idx = next_idx as usize;
            current_bit += 1;
        }

        last_match
    }
}

fn count_nodes(node: &BuildNode) -> usize {
    let left_count = node.left.as_deref().map_or(0, count_nodes);
    let right_count = node.right.as_deref().map_or(0, count_nodes);
    1 + left_count + right_count
}

struct FlatTreeBuilder {
    nodes: Vec<FlatNode>,
}

impl FlatTreeBuilder {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
        }
    }

    /// Flattens in DFS pre-order so all real children receive non-zero indices.
    fn flatten_node(&mut self, node: &BuildNode) -> Result<u32, IpRadixError> {
        if self.nodes.len() >= u32::MAX as usize {
            return Err(IpRadixError::TooManyNodes {
                count: self.nodes.len() + 1,
                max: u32::MAX as usize,
            });
        }

        let node_idx = self.nodes.len();
        self.nodes.push(FlatNode {
            left_index: 0,
            right_index: 0,
            prefix_len: node.prefix_len,
            label_id: node.label_id,
            _padding: [0; 1],
        });

        let left_idx = match node.left.as_deref() {
            Some(left) => self.flatten_node(left)?,
            None => 0,
        };
        let right_idx = match node.right.as_deref() {
            Some(right) => self.flatten_node(right)?,
            None => 0,
        };

        self.nodes[node_idx].left_index = left_idx;
        self.nodes[node_idx].right_index = right_idx;
        Ok(node_idx as u32)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/matcher/radix_ip/binary_test.rs"]
mod tests;
