//! Rebuild/check the explicit repository-owned supported CRS package.
#[allow(dead_code)]
#[path = "../tests/common/supported_crs.rs"]
mod profile;
fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    anyhow::ensure!(
        args.is_empty() || args == ["--check"],
        "usage: generate_supported_crs [--check]"
    );
    profile::generate(!args.is_empty())?;
    profile::generate_wire(!args.is_empty())
}
