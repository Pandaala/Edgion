use edgion::EdgionGatewayCli;

fn main() {
    let cli = EdgionGatewayCli::parse_args();
    if let Err(err) = cli.run() {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
