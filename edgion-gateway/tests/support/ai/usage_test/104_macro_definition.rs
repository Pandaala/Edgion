#[cfg(test)]
macro_rules! ignore_json_value_except_seq {
    ($value:expr) => {
        ignore_json_scalars!($value);

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok($value)
        }
    };
}
