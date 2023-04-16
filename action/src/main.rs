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
        let duration = value.to_ascii_lowercase();
        let duration = duration_string::DurationString::from_string(duration.clone())
            .map_err(|_| InvalidDuration(duration))?;
        Ok(Duration(duration.into()))
    }
}

// #[actions_derive::action = "../../action.yml"]
#[derive(Action)]
#[action = "../../action.yml"]
pub struct ActionD {
    value: i32,
}

// #[action("../../action.yml")]
// pub struct ActionA {}
// {
//     // value: i32,
// }

async fn run() -> eyre::Result<()> {
    color_eyre::install()?;

    // let action = ActionD { value: 0 };
    ActionD::description();

    let cwd = std::env::current_dir()?;

    let path = ActionD::path::<String>()?
        .map(PathBuf::from)
        .unwrap_or(cwd.clone());

    let path = actions::get_input::<String>("path")?
        // .transpose()
        // .map(Result::ok);
        // .map_or(Ok(None), |v| v)
        .map(PathBuf::from)
        .unwrap_or(cwd);
        // .wrap_err("path is not specified")?;

    let registry_token = actions::get_input::<String>("registry-token")?;

    let dry_run = actions::get_input::<bool>("dry-run")
        // .ok()
        // .map(parse_bool)
        // .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option dry-run")?
        .unwrap_or(false);

    let publish_delay = actions::get_input::<Duration>("publish-delay")
        // .ok()
        // .map(parse_duration)
        // .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for publish-delay")?.map(std::time::Duration::from);

    let no_verify = actions::get_input::<bool>("no-verify")
        // .ok()
        // .map(parse_bool)
        // .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option no-verify")?
        .unwrap_or(false);

    let resolve_versions = actions::get_input::<bool>("resolve-versions")
        // .ok()
        // .map(parse_bool)
        // .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option resolve-versions")?
        .unwrap_or(false);

    actions::warning!("include: {:?}", actions::get_raw_input("include"));
    actions::warning!("exclude: {:?}", actions::get_raw_input("exclude"));

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
    use actions::ParseInput;
    use std::time::Duration;

    fn parse_duration(dur: impl Into<String>) -> Option<Duration> {
        <super::Duration as ParseInput>::parse(dur.into()).ok().map(Into::into)
    }


    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("30S"), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_duration("20m"),
            Some(Duration::from_secs(20 * 60))
        );
        // todo: fix this?
        // assert_eq!(
        //     parse_duration_string("1m30s").ok(),
        //     Some(Duration::from_secs(1 * 60 + 30))
        // );
    }
}
