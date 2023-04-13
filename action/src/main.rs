// #![allow(warnings)]
use actions_derive::action;
// use actions_toolkit::prelude::*;
use color_eyre::eyre::{self, eyre, WrapErr};
use publish_crates::{publish, Options};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod actions {
    use color_eyre::eyre;
    use std::env;

    // pub struct Core<W, E> {
    //     stdout: W,
    //     stderr: E,
    // }

    #[derive(Debug)]
    pub enum LogLevel {
        Debug,
        Error,
        Warning,
    }

    impl std::fmt::Display for LogLevel {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            match self {
                LogLevel::Debug => write!(f, "debug"),
                LogLevel::Error => write!(f, "error"),
                LogLevel::Warning => write!(f, "warning"),
            }
        }
    }

    pub fn escape_data(data: impl AsRef<str>) -> String {
        data.as_ref()
            .replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
    }

    pub fn escape_property(prop: impl AsRef<str>) -> String {
        prop.as_ref()
            .replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
            .replace(':', "%3A")
            .replace(',', "%2C")
    }

    pub fn input(name: impl AsRef<str>) -> Result<String, env::VarError> {
        env::var(
            format!("INPUT_{}", name.as_ref())
                .replace(' ', "_")
                .to_uppercase(),
        )
    }

    pub fn log_message(level: LogLevel, v: impl AsRef<str>) -> std::io::Result<()> {
        println!("::{}::{}", level, escape_data(v));
        Ok(())
    }

    pub fn log_message_to(
        mut out: impl std::io::Write,
        level: LogLevel,
        v: impl AsRef<str>,
    ) -> std::io::Result<()> {
        writeln!(out, "::{}::{}", level, escape_data(v))
    }

    // pub fn set_output<K: ToString, V: ToString>(k: K, v: V) {
    //     Core::new().set_output(k, v).assert();
    // }
    //
    // pub fn set_env<K: ToString, V: ToString>(k: K, v: V) {
    //     Core::new().set_env(k, v).assert();
    // }
    //
    // pub fn add_mask<V: ToString>(v: V) {
    //     Core::new().add_mask(v).assert();
    // }

    pub fn not_empty_res(value: String) -> Result<String, std::env::VarError> {
        if value.is_empty() {
            Err(std::env::VarError::NotPresent)
        } else {
            Ok(value)
        }
    }

    pub fn parse_bool(value: impl AsRef<str>) -> eyre::Result<bool> {
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
}
use actions::*;

// #[derive(actions_derive::Action)]
// // #[actions_derive::action = "../../action.yml"]
// pub struct ActionD {
//     value: i32,
// }

#[action("../../action.yml")]
pub struct ActionA {
    value: i32,
}

fn parse_duration(duration: impl Into<String>) -> eyre::Result<Duration> {
    let duration = duration.into().to_ascii_lowercase();
    duration_string::DurationString::from_string(duration.clone())
        .map(Into::into)
        .map_err(|_| eyre!("{} is not a valid duration", &duration))
}

async fn run() -> eyre::Result<()> {
    color_eyre::install()?;

    let path: PathBuf = input("path")
        .and_then(not_empty_res)
        .map(PathBuf::from)
        .or_else(|_| std::env::current_dir())
        .wrap_err("path is not specified")?;

    let registry_token = input("registry-token").and_then(not_empty_res).ok();

    let dry_run = input("dry-run")
        .and_then(not_empty_res)
        .ok()
        .map(parse_bool)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option dry-run")?
        .unwrap_or(false);

    let publish_delay = input("publish-delay")
        .and_then(not_empty_res)
        .ok()
        .map(parse_duration)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for publish-delay")?;

    let no_verify = input("no-verify")
        .and_then(not_empty_res)
        .ok()
        .map(parse_bool)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option no-verify")?
        .unwrap_or(false);

    let resolve_versions = input("resolve-versions")
        .and_then(not_empty_res)
        .ok()
        .map(parse_bool)
        .map_or(Ok(None), |v| v.map(Some))
        .wrap_err("invalid value for option resolve-versions")?
        .unwrap_or(false);

    log_message(
        LogLevel::Warning,
        format!("include: {:?}", input("include")),
    )
    .ok();
    log_message(
        LogLevel::Warning,
        format!("exclude: {:?}", input("include")),
    )
    .ok();
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
        log_message(LogLevel::Error, format!("failed: {}", err)).ok();
        // set_failed(err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30s").ok(), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("30S").ok(), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_duration("20m").ok(),
            Some(Duration::from_secs(20 * 60))
        );
        // todo: fix this?
        // assert_eq!(
        //     parse_duration_string("1m30s").ok(),
        //     Some(Duration::from_secs(1 * 60 + 30))
        // );
    }

    #[test]
    fn test_not_empty_res() {
        use std::env::VarError;
        assert_eq!(not_empty_res("".to_string()), Err(VarError::NotPresent));
        assert_eq!(not_empty_res("test".to_string()), Ok("test".to_string()));
    }
}
