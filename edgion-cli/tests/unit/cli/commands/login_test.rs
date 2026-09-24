use crate::cli::{Cli, Commands};
use clap::Parser;

#[test]
fn cli_parses_login_with_token() {
    let cli =
        Cli::try_parse_from(["edgion-cli", "login", "--token", "edg_abc"]).expect("login with --token should parse");
    match cli.command {
        Commands::Login { token } => assert_eq!(token, "edg_abc"),
        other => panic!("expected Login, got {:?}", other),
    }
}

#[test]
fn cli_login_without_token_fails() {
    // `--token` is required; omitting it must be a parse error.
    let res = Cli::try_parse_from(["edgion-cli", "login"]);
    assert!(res.is_err(), "login without --token must fail to parse");
}

#[test]
fn cli_login_with_server_flag_at_root_parses() {
    let cli = Cli::try_parse_from([
        "edgion-cli",
        "--server",
        "http://ctl.example.com:5800",
        "login",
        "--token",
        "edg_abc",
    ])
    .expect("login with --server should parse");
    assert_eq!(cli.server.as_deref(), Some("http://ctl.example.com:5800"));
    match cli.command {
        Commands::Login { token } => assert_eq!(token, "edg_abc"),
        _ => panic!("expected Login"),
    }
}
