// #![allow(warnings)]
use actions_toolkit::prelude::*;
use color_eyre::eyre::{self, eyre, WrapErr};
use publish_crates::{publish, Options};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn parse_duration_string(duration: impl Into<String>) -> eyre::Result<Duration> {
    let duration = duration.into();
    duration_string::DurationString::from_string(duration.clone())
        .map(Into::into)
        .map_err(|_| eyre!("{} is not a valid duration", &duration))
}

fn parse_bool_string(value: impl AsRef<str>) -> eyre::Result<bool> {
    match value.as_ref().to_ascii_lowercase().as_str() {
        "yes" => Ok(true),
        "true" => Ok(true),
        "t" => Ok(true),
        "no" => Ok(false),
        "false" => Ok(false),
        "f" => Ok(false),
        _ => Err(eyre::eyre!(
            "{} can not be parsed as a boolean value",
            value.as_ref()
        )),
    }
}

async fn run() -> eyre::Result<()> {
    color_eyre::install()?;

    let path: PathBuf = input("path")
        .map(PathBuf::from)
        .or_else(|_| std::env::current_dir())?;

    let token = input("token")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .wrap_err("token is not specified")?;

    let registry_token = input("registry-token").ok();

    let dry_run = input("dry-run")
        .ok()
        .map(parse_bool_string)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option dry-run")?
        .unwrap_or(false);

    let publish_delay = input("publish-delay")
        .ok()
        .map(parse_duration_string)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for publish-delay")?;

    let no_verify = input("no-verify")
        .ok()
        .map(parse_bool_string)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option no-verify")?
        .unwrap_or(false);

    let resolve_versions = input("resolve-versions")
        .ok()
        .map(parse_bool_string)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option resolve-versions")?
        .unwrap_or(false);

    log_message(
        LogLevel::Warning,
        format!("include: {:#?}", input("include")),
    );
    log_message(
        LogLevel::Warning,
        format!("exclude: {:#?}", input("include")),
    );
    let options = Options {
        path,
        token,
        registry_token,
        dry_run,
        publish_delay,
        no_verify,
        resolve_versions,
        include: None,
        exclude: None,
    };
    publish(Arc::new(options)).await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        log_message(LogLevel::Error, format!("failed: {}", err));
        // set_failed(err);
    }
}
