use anyhow::Result;
use clap::Parser;
use publish_crates as publish;
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
    #[clap(long = "resolve-versions")]
    resolve_versions: bool,
    #[clap(long = "ignore-unpublished")]
    ignore_unpublished: bool,
    #[clap(long = "include")]
    include: Option<Vec<String>>,
    #[clap(long = "exclude")]
    exclude: Option<Vec<String>>,
}

impl Into<publish::Options> for Options {
    fn into(self) -> publish::Options {
        let working_dir = PathBuf::from(std::env::current_dir().unwrap());
        let path = self
            .path
            .as_ref()
            .map(|p| {
                if p.is_file() {
                    p.clone()
                } else {
                    p.join("Cargo.toml")
                }
            })
            .unwrap_or_else(|| working_dir.join("Cargo.toml"));

        publish::Options {
            path,
            token: self.token,
            registry_token: self.registry_token,
            dry_run: self.dry_run,
            check_repo: self.check_repo,
            publish_delay: self.publish_delay,
            no_verify: self.no_verify,
            resolve_versions: self.resolve_versions,
            ignore_unpublished: self.ignore_unpublished,
            include: self.include,
            exclude: self.exclude,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options: publish::Options = Options::parse().into();
    dbg!(&options);

    // let options = publish::Options {
    //     // path,
    //     ..options.into() // token: String,
    //                      // registry_token: Option<String>,
    //                      // dry_run: bool,
    //                      // check_repo: bool,
    //                      // publish_delay: Option<u16>,
    //                      // no_verify: bool,
    //                      // ignore_unpublished: bool,
    // };
    publish::test(&options).await?;
    Ok(())
}
