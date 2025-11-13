use edgion::core::cli::Cli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = Cli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}
