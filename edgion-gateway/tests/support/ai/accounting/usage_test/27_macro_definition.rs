#[cfg(test)]
macro_rules! ignore_json_scalars {
    ($value:expr) => {
        fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_borrowed_str<E>(self, _value: &'de str) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok($value)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok($value)
        }
    };
}
