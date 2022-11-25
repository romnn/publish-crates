use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[clap(
    name = "publish-crates",
    version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown"),
    about = "publish crates to crates.io",
    author = "romnn <contact@romnn.com>",
)]
struct Options {
    #[clap(short = 'p', long = "path")]
    path: Option<PathBuf>,
    #[clap(short = 't', long = "token")]
    token: String,
    #[clap(long = "registry-token")]
    registry_token: Option<String>,
    #[clap(long = "dry-run")]
    dry_run: bool,
    #[clap(long = "check-repo")]
    check_repo: bool,
    #[clap(long = "publish-delay")]
    publish_delay: Option<u16>,
    #[clap(long = "no-verify")]
    no_verify: bool,
    #[clap(long = "ignore-unpublished")]
    ignore_unpublished: bool,
}

fn main() {
    let options = Options::parse();
    dbg!(&options);
}
