#[cfg(test)]
macro_rules! ignore_json_value_except_map {
    ($value:expr) => {
        ignore_json_scalars!($value);

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            while sequence.next_element::<IgnoredAny>()?.is_some() {}
            Ok($value)
        }
    };
}
