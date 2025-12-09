use edgion::EdgionGwCli;

fn main() {
    let cli = EdgionGwCli::parse_args();
    if let Err(err) = cli.run() {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
