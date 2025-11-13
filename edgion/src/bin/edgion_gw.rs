use edgion::EdgionGwCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = EdgionGwCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

