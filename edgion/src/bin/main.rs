use edgion::core::cli::Cli;

fn main() {
    let cli = Cli::parse_args();
    cli.validate_and_run();
}
