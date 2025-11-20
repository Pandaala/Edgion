use edgion::EdgionOpCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = EdgionOpCli::parse_args();
    if let Err(err) = cli.run().await {
        tracing::error!("{}", err);
        std::process::exit(1);
    }
}
