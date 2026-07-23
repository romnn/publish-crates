//! Command-line interface for publishing interdependent Cargo workspace packages.

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
    #[clap(last = true, value_name = "CARGO_PUBLISH_ARGS")]
    extra_args: Vec<String>,
}

impl TryFrom<Options> for publish::Options {
    type Error = std::io::Error;

    fn try_from(options: Options) -> Result<Self, Self::Error> {
        let working_dir = std::env::current_dir()?;
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

        Ok(publish::Options {
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
            extra_args: options.extra_args,
        })
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let options: publish::Options = Options::parse().try_into()?;
    publish::publish(options).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Options;
    use clap::Parser;
    use similar_asserts::assert_eq as sim_assert_eq;

    #[test]
    fn parses_selection_and_cargo_arguments() {
        let options = Options::try_parse_from([
            "cargo-publish-crates",
            "--dry-run",
            "--include",
            "core",
            "--exclude",
            "internal",
            "--",
            "--registry",
            "private",
        ])
        .expect("arguments must parse");

        sim_assert_eq!(options.include, Some(vec!["core".to_string()]));
        sim_assert_eq!(options.exclude, Some(vec!["internal".to_string()]));
        sim_assert_eq!(
            options.extra_args,
            vec!["--registry".to_string(), "private".to_string()]
        );
        assert!(options.dry_run);
    }

    #[test]
    fn converts_directory_and_manifest_paths() {
        let temp = tempfile::tempdir().expect("temporary directory must be created");
        let manifest = temp.path().join("Cargo.toml");
        std::fs::write(&manifest, "[workspace]\n").expect("temporary manifest must be writable");

        let directory_options = Options::try_parse_from([
            "cargo-publish-crates",
            "--path",
            temp.path().to_str().expect("temporary path must be UTF-8"),
        ])
        .expect("directory arguments must parse");
        let directory_options =
            publish_crates::Options::try_from(directory_options).expect("current directory exists");

        let manifest_options = Options::try_parse_from([
            "cargo-publish-crates",
            "--path",
            manifest.to_str().expect("manifest path must be UTF-8"),
        ])
        .expect("manifest arguments must parse");
        let manifest_options =
            publish_crates::Options::try_from(manifest_options).expect("current directory exists");

        sim_assert_eq!(directory_options.path, manifest);
        sim_assert_eq!(manifest_options.path, manifest);
    }
}
