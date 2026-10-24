/// Yield set-bit indices in ascending order within the logical bit count.
pub(crate) fn set_bit_indices(words: &[u64], bit_count: usize) -> impl Iterator<Item = usize> + '_ {
    let bits = u64::BITS as usize;
    words
        .iter()
        .take(bit_count.div_ceil(bits))
        .enumerate()
        .flat_map(move |(word_index, &word)| {
            let mut remaining = word;
            std::iter::from_fn(move || {
                if remaining == 0 {
                    return None;
                }
                let bit = remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                Some(word_index * bits + bit)
            })
        })
        .take_while(move |&index| index < bit_count)
}
