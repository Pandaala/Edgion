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

#[test]
fn membership_stops_at_first_terminal_for_both_key_widths() {
    use std::cell::Cell;

    #[derive(Clone, Copy)]
    struct CountedKey<'a, K> {
        key: K,
        reads: &'a Cell<usize>,
    }
    impl<K: BitKey> BitKey for CountedKey<'_, K> {
        const WIDTH: u8 = K::WIDTH;

        fn bit_at(self, depth: u8) -> bool {
            self.reads.set(self.reads.get() + 1);
            self.key.bit_at(depth)
        }
    }
    fn check<K: BitKey>(key: K, first_prefix: u8) {
        let reads = Cell::new(0);
        let counted = CountedKey { key, reads: &reads };
        let mut builder = BinaryRadixBuilder::new();
        builder.insert(counted, first_prefix, 1, "broad").unwrap();
        builder.insert(counted, K::WIDTH, 2, "host").unwrap();
        let tree = builder.freeze().unwrap();
        reads.set(0);
        assert_eq!(tree.match_key::<true>(counted), Some(1));
        assert_eq!(reads.get(), usize::from(first_prefix));
        reads.set(0);
        assert_eq!(tree.match_key::<false>(counted), Some(2));
        assert_eq!(reads.get(), usize::from(K::WIDTH));
    }
    check(0xc0000207_u32, 0);
    check(0xc0000207_u32, 24);
    check(0x20010db8000000000000000000000007_u128, 0);
    check(0x20010db8000000000000000000000007_u128, 32);
}
