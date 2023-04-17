#![allow(warnings)]

use actions::Action;
use color_eyre::eyre::{self, eyre, WrapErr};
use publish_crates::{publish, Options};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Duration(std::time::Duration);

impl From<Duration> for std::time::Duration {
    fn from(dur: Duration) -> Self {
        dur.0
    }
}

#[derive(thiserror::Error, Debug)]
#[error("{0} is not a valid duraition")]
pub struct InvalidDuration(String);

impl actions::ParseInput for Duration {
    type Error = InvalidDuration;

    fn parse(value: String) -> Result<Self, Self::Error> {
        let dur = value.to_ascii_lowercase();
        let dur = duration_string::DurationString::from_string(dur.clone())
            .map_err(|_| InvalidDuration(dur))?;
        Ok(Duration(dur.into()))
    }
}

#[derive(Action)]
#[action = "../../action.yml"]
pub struct PublishCratesAction {
    _test: String,
}

async fn run() -> eyre::Result<()> {
    color_eyre::install()?;

    let cwd = std::env::current_dir()?;

    let path = PublishCratesAction::path::<String>()?
        .map(PathBuf::from)
        .unwrap_or(cwd.clone());

    let path = actions::get_input::<String>("path")?
        .map(PathBuf::from)
        .unwrap_or(cwd);

    let registry_token = PublishCratesAction::registry_token::<String>()?;

    let dry_run = PublishCratesAction::dry_run::<bool>()
        .wrap_err("invalid value for option dry-run")?
        .unwrap_or(false);

    let publish_delay = PublishCratesAction::publish_delay::<Duration>()
        .wrap_err("invalid value for publish-delay")?
        .map(std::time::Duration::from);

    let no_verify = PublishCratesAction::no_verify::<bool>()
        .wrap_err("invalid value for option no-verify")?
        .unwrap_or(false);

    let resolve_versions = PublishCratesAction::resolve_versions::<bool>()
        .wrap_err("invalid value for option resolve-versions")?
        .unwrap_or(false);

    actions::info!("include: {:?}", actions::get_input::<String>("include"));
    actions::info!("exclude: {:?}", actions::get_input::<String>("exclude"));

    let options = Options {
        path,
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
        actions::fail(err);
    }
}

#[cfg(test)]
mod tests {
    use super::{PublishCratesAction as Action, PublishCratesActionInput as Input};
    use actions::{Env, Parse, ParseInput, ReadEnv};
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::time::Duration;

    fn parse_duration(dur: impl Into<String>) -> Option<Duration> {
        <super::Duration as ParseInput>::parse(dur.into())
            .ok()
            .map(Into::into)
    }

    #[test]
    fn test_common_config() -> Result<()> {
        let env = Env::from_str(
            "
registry-token: test-token
resolve-versions: true
publish-delay: 30s",
        )?;
        let config = Action::parse(&env);
        dbg!(&config);
        // let resolve = actions::get_input_from::<bool>(&env, "resolve-versions");
        // dbg!(&resolve);
        // assert_eq!(resolve, Ok(Some(true)));
        assert_eq!(
            config,
            HashMap::from_iter([
                (Input::Token, Some("${{ github.token }}".to_string())),
                (Input::Version, None),
                (Input::DryRun, Some("false".to_string())),
                (Input::Path, Some(".".to_string())),
                (Input::RegistryToken, Some("test-token".to_string())),
                (Input::ExtraArgs, None),
                (Input::ResolveVersions, Some("true".to_string())),
                (Input::Include, None),
                (Input::NoVerify, Some("false".to_string())),
                (Input::Exclude, None),
                (Input::PublishDelay, Some("30s".to_string())),
            ])
        );
        Ok(())
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("30S"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("20m"), Some(Duration::from_secs(20 * 60)));
        // todo: fix this?
        // assert_eq!(
        //     parse_duration_string("1m30s").ok(),
        //     Some(Duration::from_secs(1 * 60 + 30))
        // );
    }
}
