use super::*;
#[test]
fn collect_str_streams_into_the_capped_writer() {
    struct Formatted;
    impl std::fmt::Display for Formatted {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for _ in 0..100 {
                f.write_str("abcdefgh")?;
            }
            Ok(())
        }
    }
    impl Serialize for Formatted {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.collect_str(self)
        }
    }
    let mut writer = crate::grpc_transcoding::CappedWriter::new(16);
    assert!(serde_json::to_writer(&mut writer, &ProtoJson(&Formatted)).is_err());
    assert!(writer.into_bytes().len() <= 16);
    assert_eq!(
        serde_json::to_string(&ProtoJson(&vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.25])).unwrap(),
        r#"["NaN","Infinity","-Infinity",1.25]"#
    );
}
