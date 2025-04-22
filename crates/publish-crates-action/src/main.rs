use action_core::{self as action, Action};
use color_eyre::eyre::{self, WrapErr};
use publish_crates::{Options, publish};
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

impl action::ParseInput for Duration {
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
pub struct PublishCratesAction;

async fn run() -> eyre::Result<()> {
    color_eyre::install()?;

    let cwd = std::env::current_dir()?;

    let path = PublishCratesAction::path::<String>()?.map_or(cwd, PathBuf::from);

    let registry_token = PublishCratesAction::registry_token::<String>()?;

    let dry_run = PublishCratesAction::dry_run::<bool>()
        .wrap_err("invalid value for option dry-run")?
        .unwrap_or(false);

    let publish_delay = PublishCratesAction::publish_delay::<Duration>()
        .wrap_err("invalid value for publish-delay")?
        .map(std::time::Duration::from);

    let max_retries = PublishCratesAction::max_retries::<String>()?
        .as_deref()
        .map(str::parse)
        .transpose()
        .wrap_err("invalid value for max-retries")?;

    let concurrency_limit = PublishCratesAction::concurrency_limit::<String>()?
        .as_deref()
        .map(str::parse)
        .transpose()
        .wrap_err("invalid value for concurrency-limit")?;

    let no_verify = PublishCratesAction::no_verify::<bool>()
        .wrap_err("invalid value for option no-verify")?
        .unwrap_or(false);

    let resolve_versions = PublishCratesAction::resolve_versions::<bool>()
        .wrap_err("invalid value for option resolve-versions")?
        .unwrap_or(false);

    action::info!("include: {:?}", PublishCratesAction::include::<String>());
    action::info!("exclude: {:?}", PublishCratesAction::exclude::<String>());

    let options = Arc::new(Options {
        path,
        registry_token,
        dry_run,
        publish_delay,
        max_retries,
        concurrency_limit,
        no_verify,
        resolve_versions,
        include: None,
        exclude: None,
    });
    publish(options).await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        action::fail(err);
    }
}

#[cfg(test)]
mod tests {
    use super::{PublishCratesAction as Action, PublishCratesActionInput as Input};
    use action_core::{self as action, Parse, ParseInput};
    use color_eyre::eyre;
    use itertools::Itertools;
    use similar_asserts::assert_eq as sim_assert_eq;
    use std::str::FromStr;
    use std::time::Duration;

    fn parse_duration(dur: impl Into<String>) -> Option<Duration> {
        <super::Duration as ParseInput>::parse(dur.into())
            .ok()
            .map(Into::into)
    }

    #[test]
    fn test_common_config() -> eyre::Result<()> {
        let env = action::env::Env::from_str(
            "
registry-token: test-token
resolve-versions: true
publish-delay: 30s",
        )?;
        let config: Vec<_> = Action::parse_from(&env)
            .into_iter()
            .sorted_by_key(|(input, _)| format!("{:?}", input))
            .collect();
        dbg!(&config);
        sim_assert_eq!(
            config,
            [
                (Input::Token, Some("${{ github.token }}".to_string())),
                (Input::Version, None),
                (Input::DryRun, Some("false".to_string())),
                (Input::Path, Some(".".to_string())),
                (Input::RegistryToken, Some("test-token".to_string())),
                (Input::MaxRetries, Some("5".to_string())),
                (Input::ConcurrencyLimit, Some("4".to_string())),
                (Input::ExtraArgs, None),
                (Input::ResolveVersions, Some("true".to_string())),
                (Input::Include, None),
                (Input::NoVerify, Some("false".to_string())),
                (Input::Exclude, None),
                (Input::PublishDelay, Some("30s".to_string())),
            ]
            .into_iter()
            .sorted_by_key(|(input, _)| format!("{:?}", input))
            .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn test_parse_duration() {
        sim_assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        sim_assert_eq!(parse_duration("30S"), Some(Duration::from_secs(30)));
        sim_assert_eq!(parse_duration("20m"), Some(Duration::from_secs(20 * 60)));
        // todo: fix this?
        // assert_eq!(
        //     parse_duration_string("1m30s").ok(),
        //     Some(Duration::from_secs(1 * 60 + 30))
        // );
    }
}
