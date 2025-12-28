use action_core::{self as action, Action};
use color_eyre::eyre::{self, WrapErr};
use publish_crates::{Options, publish};
use std::ffi::OsString;
use std::path::PathBuf;

pub struct Duration(std::time::Duration);

impl From<Duration> for std::time::Duration {
    fn from(dur: Duration) -> Self {
        dur.0
    }
}

#[derive(thiserror::Error, Debug)]
#[error("{0:?} is not a valid duraition")]
pub struct InvalidDuration(OsString);

impl action::input::Parse for Duration {
    type Error = InvalidDuration;

    fn parse(value: OsString) -> Result<Self, Self::Error> {
        use std::str::FromStr;
        let dur = value.to_ascii_lowercase();
        let dur = duration_string::DurationString::from_str(&dur.to_string_lossy())
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

    let options = Options {
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
    };
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
    use super::{PublishCratesAction, PublishCratesActionInput};
    use action_core::{self as action, Parse, input};
    use color_eyre::eyre;
    use indoc::indoc;
    use itertools::Itertools;
    use similar_asserts::assert_eq as sim_assert_eq;
    use std::ffi::OsString;
    use std::time::Duration;

    fn parse_duration(dur: impl Into<OsString>) -> Option<Duration> {
        <super::Duration as input::Parse>::parse(dur.into())
            .ok()
            .map(Into::into)
    }

    #[test]
    fn test_common_config() -> eyre::Result<()> {
        use input::SetInput;
        use std::collections::HashMap;

        color_eyre::install()?;

        let config = indoc! {"
            registry-token: test-token
            resolve-versions: true
            publish-delay: 30s"
        };
        let config = serde_yaml::from_str::<HashMap<String, String>>(config)?;
        dbg!(&config);

        let env = action::env::EnvMap::default();
        for (k, v) in config {
            env.set_input(k, v);
        }
        dbg!(&env);

        let config: Vec<_> = PublishCratesAction::parse_from(&env)
            .into_iter()
            .sorted_by_key(|(input, _)| format!("{input:?}"))
            .collect();

        let expected = [
            (
                PublishCratesActionInput::Token,
                Some("${{ github.token }}".to_string()),
            ),
            (
                PublishCratesActionInput::Version,
                Some("latest".to_string()),
            ),
            (PublishCratesActionInput::DryRun, Some("false".to_string())),
            (PublishCratesActionInput::Path, Some(".".to_string())),
            (
                PublishCratesActionInput::RegistryToken,
                Some("test-token".to_string()),
            ),
            (PublishCratesActionInput::MaxRetries, None),
            (
                PublishCratesActionInput::ConcurrencyLimit,
                Some("4".to_string()),
            ),
            (PublishCratesActionInput::ExtraArgs, None),
            (
                PublishCratesActionInput::ResolveVersions,
                Some("true".to_string()),
            ),
            (PublishCratesActionInput::Include, None),
            (
                PublishCratesActionInput::NoVerify,
                Some("false".to_string()),
            ),
            (PublishCratesActionInput::Exclude, None),
            (
                PublishCratesActionInput::PublishDelay,
                Some("30s".to_string()),
            ),
        ];
        sim_assert_eq!(
            config,
            expected
                .into_iter()
                .sorted_by_key(|(input, _)| format!("{input:?}"))
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
