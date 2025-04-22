use clap::Parser;
use color_eyre::eyre::{self, eyre};
use publish_crates as publish;
use std::path::PathBuf;
use std::time::Duration;

fn parse_duration_string(duration: &str) -> eyre::Result<Duration> {
    duration_string::DurationString::from_string(duration.into())
        .map(Into::into)
        .map_err(|_| eyre!("{} is not a valid duration", duration))
}

const ENV_PREFIX: &str = "PUBLISH_CRATES";

#[derive(Parser, Debug, Clone)]
#[clap(
    name = "publish-crates",
    version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown"),
    about = "publish crates to crates.io",
    author = "romnn <contact@romnn.com>",
)]
struct Options {
    #[clap(short = 'p', long = "path", env = format!("{ENV_PREFIX}_CRATE_PATH"))]
    path: Option<PathBuf>,
    #[clap(long = "registry-token", env = format!("{ENV_PREFIX}_REGISTRY_TOKEN"))]
    registry_token: Option<String>,
    #[clap(long = "dry-run", env = format!("{ENV_PREFIX}_DRY_RUN"))]
    dry_run: bool,
    #[clap(
        long = "publish-delay",
        env = format!("{ENV_PREFIX}_PUBLISH_DELAY"),
        value_parser = parse_duration_string,
    )]
    publish_delay: Option<Duration>,
    #[clap(long = "max-retries", env = format!("{ENV_PREFIX}_MAX_RETRIES"))]
    max_retries: Option<usize>,
    #[clap(long = "concurrency-limit", env = format!("{ENV_PREFIX}_CONCURRENCY_LIMIT"))]
    concurrency_limit: Option<usize>,
    #[clap(long = "no-verify", env = format!("{ENV_PREFIX}_NO_VERIFY"))]
    no_verify: bool,
    #[clap(long = "resolve-versions", env = format!("{ENV_PREFIX}_RESOLVE_VERSIONS"))]
    resolve_versions: bool,
    #[clap(long = "include", env = format!("{ENV_PREFIX}_INCLUDE_PACKAGES"))]
    include: Option<Vec<String>>,
    #[clap(long = "exclude", env = format!("{ENV_PREFIX}_EXCLUDE_PACKAGES"))]
    exclude: Option<Vec<String>>,
}

impl From<Options> for publish::Options {
    fn from(options: Options) -> Self {
        let working_dir = std::env::current_dir().unwrap();
        let path = options.path.as_ref().map_or_else(
            || working_dir.join("Cargo.toml"),
            |p| {
                if p.is_file() {
                    p.clone()
                } else {
                    p.join("Cargo.toml")
                }
            },
        );

        publish::Options {
            path,
            registry_token: options.registry_token,
            dry_run: options.dry_run,
            publish_delay: options.publish_delay,
            max_retries: options.max_retries,
            concurrency_limit: options.concurrency_limit,
            no_verify: options.no_verify,
            resolve_versions: options.resolve_versions,
            include: options.include,
            exclude: options.exclude,
        }
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let options: publish::Options = Options::parse().into();
    publish::publish(options).await?;
    Ok(())
}
