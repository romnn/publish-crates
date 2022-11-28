// #![allow(warnings)]
use clap::Parser;
use color_eyre::eyre::{self, eyre};
use publish_crates as publish;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn parse_duration_string(duration: &str) -> eyre::Result<Duration> {
    duration_string::DurationString::from_string(duration.into())
        .map(Into::into)
        .map_err(|_| eyre!("{} is not a valid duration", duration))
}

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
    #[clap(long = "registry-token")]
    registry_token: Option<String>,
    #[clap(long = "dry-run")]
    dry_run: bool,
    #[clap(long = "publish-delay", value_parser = parse_duration_string)]
    publish_delay: Option<Duration>,
    #[clap(long = "no-verify")]
    no_verify: bool,
    #[clap(long = "resolve-versions")]
    resolve_versions: bool,
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
            registry_token: self.registry_token,
            dry_run: self.dry_run,
            publish_delay: self.publish_delay.into(),
            no_verify: self.no_verify,
            resolve_versions: self.resolve_versions,
            include: self.include,
            exclude: self.exclude,
        }
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let options: publish::Options = Options::parse().into();
    publish::publish(Arc::new(options)).await.unwrap();
    Ok(())
}
