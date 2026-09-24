use super::*;
use clap::Parser;

/// Helper Cli that wraps GenCertsArgs as a flat command for parsing tests.
/// This avoids depending on the full `cli::Commands` subcommand structure.
#[derive(Parser, Debug)]
#[command(name = "gen-certs-test")]
struct TestCli {
    #[command(flatten)]
    args: GenCertsArgs,
}

fn parse(argv: &[&str]) -> Result<TestCli, clap::Error> {
    TestCli::try_parse_from(argv)
}

#[test]
fn output_mode_value_enum_file_variant_parses() {
    let cli = parse(&["gen-certs-test", "--output", "file"]).expect("parse should succeed");
    assert!(matches!(cli.args.output, OutputMode::File));
}

#[test]
fn output_mode_value_enum_k8s_variant_parses() {
    let cli = parse(&["gen-certs-test", "--output", "k8s"]).expect("parse should succeed");
    assert!(matches!(cli.args.output, OutputMode::K8s));
}

#[test]
fn output_mode_rejects_unknown_variant() {
    let err = parse(&["gen-certs-test", "--output", "stdout"]).expect_err("unknown variant should fail");
    assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
}

#[test]
fn defaults_are_applied_when_no_args_given() {
    let cli = parse(&["gen-certs-test"]).expect("default-only parse should succeed");
    assert!(matches!(cli.args.output, OutputMode::File));
    assert_eq!(cli.args.output_dir, PathBuf::from("certs/grpc"));
    assert_eq!(cli.args.cert_name, "primary");
    assert_eq!(cli.args.secret_prefix, "edgion");
    assert_eq!(cli.args.ca_days, 3650);
    assert_eq!(cli.args.cert_days, 365);
    assert!(cli.args.namespace.is_none());
    assert!(cli.args.server_san.is_empty());
    assert!(!cli.args.force);
}

#[test]
fn server_san_accepts_multiple_occurrences() {
    let cli = parse(&[
        "gen-certs-test",
        "--server-san",
        "ctl.example.com",
        "--server-san",
        "ctl.internal",
    ])
    .expect("parse should succeed");
    assert_eq!(cli.args.server_san, vec!["ctl.example.com", "ctl.internal"]);
}

#[test]
fn force_flag_is_boolean_with_no_value() {
    let cli = parse(&["gen-certs-test", "--force"]).expect("parse should succeed");
    assert!(cli.args.force);
}

#[test]
fn namespace_short_flag_n_is_accepted() {
    let cli = parse(&["gen-certs-test", "-n", "edgion-system"]).expect("parse should succeed");
    assert_eq!(cli.args.namespace.as_deref(), Some("edgion-system"));
}

#[test]
fn ca_days_rejects_non_numeric_value() {
    let err = parse(&["gen-certs-test", "--ca-days", "forever"]).expect_err("non-numeric should fail");
    assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
}

#[test]
fn cert_days_accepts_custom_value() {
    let cli = parse(&["gen-certs-test", "--cert-days", "30"]).expect("parse should succeed");
    assert_eq!(cli.args.cert_days, 30);
}

#[test]
fn validate_cert_name_accepts_valid_rotation_names() {
    for name in ["primary", "secondary", "rotation-2", "ca0"] {
        assert!(validate_cert_name(name).is_ok(), "{name:?} should be accepted");
    }
}

#[test]
fn validate_cert_name_rejects_path_traversal() {
    for name in ["..", "../x", "../../outside", "a/b", "a\\b", "./x"] {
        assert!(validate_cert_name(name).is_err(), "{name:?} should be rejected");
    }
}

#[test]
fn validate_cert_name_rejects_empty_and_control_chars() {
    assert!(validate_cert_name("").is_err());
    assert!(validate_cert_name("a b").is_err());
    assert!(validate_cert_name("a\tb").is_err());
    assert!(validate_cert_name("name\n").is_err());
}

#[test]
fn validate_cert_name_rejects_bad_boundaries_and_uppercase() {
    // Leading/trailing hyphen and uppercase are not valid DNS-1123 labels.
    assert!(validate_cert_name("-primary").is_err());
    assert!(validate_cert_name("primary-").is_err());
    assert!(validate_cert_name("Primary").is_err());
}

#[test]
fn validate_cert_name_rejects_overlong_value() {
    let long = "a".repeat(64);
    assert!(validate_cert_name(&long).is_err());
    let ok = "a".repeat(63);
    assert!(validate_cert_name(&ok).is_ok());
}

#[test]
fn detect_namespace_from_sa_returns_none_when_file_missing() {
    // The SA path is highly unlikely to exist on a developer machine running
    // unit tests; if it does, we accept either outcome since the fn is best-effort.
    let result = detect_namespace_from_sa();
    if std::path::Path::new("/var/run/secrets/kubernetes.io/serviceaccount/namespace").exists() {
        // Inside a Pod — value should be Some(non-empty)
        assert!(result.as_ref().is_some_and(|s| !s.is_empty()));
    } else {
        assert!(result.is_none());
    }
}
