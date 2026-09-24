use super::*;

#[test]
fn missing_or_blank_identity_fails_before_starting_the_source() {
    assert!(FileSystemCenter::new(FileSystemConfig::default()).is_err());
    assert!(FileSystemCenter::new(FileSystemConfig::default().with_controller_name(" ")).is_err());
}

#[test]
fn new_absolutizes_conf_dir_and_shares_base_with_storage() {
    let center = FileSystemCenter::new(
        FileSystemConfig::new(std::path::PathBuf::from("relative/conf"))
            .with_controller_name("edgion.io/gateway-controller"),
    )
    .unwrap();
    assert!(
        center.conf_dir_abs().is_absolute(),
        "conf_dir handed to the watcher must be absolute"
    );
    assert_eq!(
        center.storage_root(),
        center.conf_dir_abs(),
        "storage scan base must equal the watcher base (design §0)"
    );
}
