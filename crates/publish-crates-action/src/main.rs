//! GitHub Actions entry point for publishing interdependent Cargo workspace packages.

use action_core::{self as action};
use color_eyre::eyre::{self, WrapErr};
use publish_crates::{Options, publish};
use std::ffi::OsString;
use std::path::PathBuf;

fn parse_package_names(value: Option<String>) -> Option<Vec<String>> {
    let packages = value?
        .split([',', ' ', '\t', '\n', '\r'])
        .filter(|package| !package.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!packages.is_empty()).then_some(packages)
}

fn parse_extra_args(value: Option<String>) -> eyre::Result<Vec<String>> {
    value.map_or_else(
        || Ok(Vec::new()),
        |args| {
            shlex::split(&args).ok_or_else(|| eyre::eyre!("extra-args contains unmatched quotes"))
        },
    )
}

struct Duration(std::time::Duration);

impl From<Duration> for std::time::Duration {
    fn from(dur: Duration) -> Self {
        dur.0
    }
}

#[derive(thiserror::Error, Debug)]
#[error("{0:?} is not a valid duration")]
struct InvalidDuration(OsString);

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

mod action_definition {
    use action_core::Action;

    #[derive(Action)]
    #[action = "../../action.yml"]
    pub(super) struct PublishCratesAction;
}

use action_definition::PublishCratesAction;
#[cfg(test)]
use action_definition::PublishCratesActionInput;

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

    let include = parse_package_names(PublishCratesAction::include::<String>()?);
    let exclude = parse_package_names(PublishCratesAction::exclude::<String>()?);
    let extra_args = parse_extra_args(PublishCratesAction::extra_args::<String>()?)?;

    action::info!("include: {include:?}");
    action::info!("exclude: {exclude:?}");

    let options = Options {
        path,
        registry_token,
        dry_run,
        publish_delay,
        max_retries,
        concurrency_limit,
        no_verify,
        resolve_versions,
        include,
        exclude,
        extra_args,
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
    use super::{
        PublishCratesAction, PublishCratesActionInput, parse_extra_args, parse_package_names,
    };
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
    fn action_schema_applies_defaults_and_overrides() -> eyre::Result<()> {
        use input::SetInput;
        use std::collections::HashMap;

        color_eyre::install()?;

        let config = indoc! {"
            registry-token: test-token
            resolve-versions: true
            publish-delay: 30s"
        };
        let config = serde_yaml::from_str::<HashMap<String, String>>(config)?;

        let env = action::env::EnvMap::default();
        for (k, v) in config {
            env.set_input(k, v);
        }

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
    fn action_forwards_every_publishing_input() -> eyre::Result<()> {
        let action =
            serde_yaml::from_str::<serde_yaml::Value>(include_str!("../../../action.yml"))?;
        let publish_step = action
            .get("runs")
            .and_then(|runs| runs.get("steps"))
            .and_then(serde_yaml::Value::as_sequence)
            .and_then(|steps| {
                steps.iter().find(|step| {
                    step.get("name").and_then(serde_yaml::Value::as_str) == Some("Publish crates")
                })
            })
            .ok_or_else(|| eyre::eyre!("publish step is missing"))?;
        let forwarded = publish_step
            .get("env")
            .and_then(serde_yaml::Value::as_mapping)
            .ok_or_else(|| eyre::eyre!("publish step environment is missing"))?;
        let expected = serde_yaml::from_str::<serde_yaml::Mapping>(indoc! {"
            INPUT_TOKEN: ${{ inputs.token }}
            INPUT_PATH: ${{ inputs.path }}
            INPUT_INCLUDE: ${{ inputs.include }}
            INPUT_EXCLUDE: ${{ inputs.exclude }}
            INPUT_EXTRA-ARGS: ${{ inputs.extra-args }}
            INPUT_REGISTRY-TOKEN: ${{ inputs.registry-token }}
            INPUT_DRY-RUN: ${{ inputs.dry-run }}
            INPUT_PUBLISH-DELAY: ${{ inputs.publish-delay }}
            INPUT_CONCURRENCY-LIMIT: ${{ inputs.concurrency-limit }}
            INPUT_MAX-RETRIES: ${{ inputs.max-retries }}
            INPUT_NO-VERIFY: ${{ inputs.no-verify }}
            INPUT_RESOLVE-VERSIONS: ${{ inputs.resolve-versions }}
        "})?;

        sim_assert_eq!(forwarded, &expected);
        Ok(())
    }

    #[test]
    fn duration_parser_is_case_insensitive() {
        sim_assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        sim_assert_eq!(parse_duration("30S"), Some(Duration::from_secs(30)));
        sim_assert_eq!(parse_duration("20m"), Some(Duration::from_mins(20)));
        sim_assert_eq!(parse_duration("1m30s"), Some(Duration::from_secs(90)));
    }

    #[test]
    fn package_names_accept_commas_and_whitespace() {
        sim_assert_eq!(
            parse_package_names(Some("core, cli\naction".to_string())),
            Some(vec![
                "core".to_string(),
                "cli".to_string(),
                "action".to_string()
            ])
        );
        sim_assert_eq!(parse_package_names(Some(" , \t".to_string())), None);
        sim_assert_eq!(parse_package_names(None), None);
    }

    #[test]
    fn extra_args_preserve_quoted_values() -> eyre::Result<()> {
        sim_assert_eq!(
            parse_extra_args(Some(
                "--registry private --config 'net.retry = 2'".to_string()
            ))?,
            vec![
                "--registry".to_string(),
                "private".to_string(),
                "--config".to_string(),
                "net.retry = 2".to_string()
            ]
        );
        sim_assert_eq!(parse_extra_args(None)?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn extra_args_reject_unmatched_quotes() {
        let error = parse_extra_args(Some("'unfinished".to_string()))
            .expect_err("unmatched quotes must be rejected");
        sim_assert_eq!(error.to_string(), "extra-args contains unmatched quotes");
    }
}
