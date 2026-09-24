use super::*;
use crate::matcher::radix_ip::{FrozenIpV4RadixTree, FrozenIpV6RadixTree};

#[test]
fn flat_node_layout_remains_compact() {
    assert_eq!(std::mem::size_of::<FlatNode>(), 16);
    assert_eq!(std::mem::align_of::<FlatNode>(), 8);
    assert_eq!(
        std::mem::size_of::<FrozenBinaryRadixTree<u32>>(),
        std::mem::size_of::<Vec<FlatNode>>()
    );
    assert_eq!(
        std::mem::size_of::<FrozenBinaryRadixTree<u128>>(),
        std::mem::size_of::<Vec<FlatNode>>()
    );
    assert_eq!(
        std::mem::size_of::<FrozenIpV4RadixTree>(),
        std::mem::size_of::<Vec<FlatNode>>()
    );
    assert_eq!(
        std::mem::size_of::<FrozenIpV6RadixTree>(),
        std::mem::size_of::<Vec<FlatNode>>()
    );
}

#[test]
fn flattening_preserves_dfs_indices_and_zero_sentinel() {
    let mut builder = BinaryRadixBuilder::<u32>::new();
    builder.insert(0, 1, 1, "0.0.0.0/1").unwrap();
    builder.insert(1 << 31, 1, 2, "128.0.0.0/1").unwrap();

    let tree = builder.freeze().unwrap();
    assert_eq!(
        tree.nodes,
        vec![
            FlatNode {
                left_index: 1,
                right_index: 2,
                prefix_len: 0,
                label_id: 0,
                _padding: [0],
            },
            FlatNode {
                left_index: 0,
                right_index: 0,
                prefix_len: 1,
                label_id: 1,
                _padding: [0],
            },
            FlatNode {
                left_index: 0,
                right_index: 0,
                prefix_len: 1,
                label_id: 2,
                _padding: [0],
            },
        ]
    );
}
