//! Classification of `cargo publish` failures from Cargo's diagnostics.

/// Environment variable that overrides Cargo's `resolver.incompatible-publish-age` setting.
pub(crate) const INCOMPATIBLE_PUBLISH_AGE_ENV: &str = "CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE";

/// A dependency whose releases Cargo rejected as newer than the registry's `min-publish-age`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TooNewDependency<'a> {
    pub(crate) package: &'a str,
    /// Every release that matched the requirement but was rejected for its age.
    pub(crate) versions: Vec<&'a str>,
}

/// Returns the dependency whose releases Cargo rejected under the registry's `min-publish-age`.
///
/// Only this diagnostic marks the rejection as permanent.
/// Other resolution failures, such as a version or feature the registry index does not list yet,
/// stay retryable.
pub(crate) fn too_new_dependency(stderr: &str) -> Option<TooNewDependency<'_>> {
    const REQUIREMENT: &str = "failed to select a version for the requirement `";

    // Cargo names the requirement, then lists each rejected candidate on a line of its own.
    let (_, requirement) = stderr.split_once(REQUIREMENT)?;
    let (package, candidates) = requirement.split_once(" = ")?;
    let versions: Vec<&str> = candidates
        .lines()
        .filter_map(|line| {
            let candidate = line.trim_start().strip_prefix("version ")?;
            let (version, _) = candidate.split_once(" is too new (published ")?;
            Some(version)
        })
        .collect();
    if versions.is_empty() {
        return None;
    }
    Some(TooNewDependency { package, versions })
}

/// Returns whether `cargo publish` failed because the package version is already published.
///
/// A dry run reports an existing version only as a warning and goes on, so a later failure in the
/// same output is real.
/// The error is not anchored to the line start, because stripped progress output can precede it.
pub(crate) fn is_already_published(stderr: &str) -> bool {
    stderr.lines().any(|line| {
        line.contains("error: crate ") && line.contains(" already exists on crates.io index")
    })
}

/// Classification of publishing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PublishError {
    /// An error without a recognized HTTP status code.
    Unknown,
    /// A transient HTTP error that may succeed when retried.
    Retryable(http::StatusCode),
    /// An HTTP error that should not be retried.
    Fatal(http::StatusCode),
}

impl PublishError {
    /// Returns the recognized HTTP status code, if one was found.
    #[must_use]
    pub fn code(&self) -> Option<&http::StatusCode> {
        match self {
            Self::Unknown => None,
            Self::Retryable(code) | Self::Fatal(code) => Some(code),
        }
    }
}

/// Classifies a `cargo publish` error from HTTP status text.
///
/// This approach assumes that the error messages of `cargo publish` include network errors
/// in the form `<code> <canonical_reason>`.
///
/// Classification:
///
/// - [`PublishError::Retryable`] for temporary or intermittent errors.
/// - [`PublishError::Fatal`] for permanent errors such as missing permissions.
/// - [`PublishError::Unknown`] when no recognized status text is present.
pub(crate) fn classify_publish_error(text: &str) -> PublishError {
    for code_num in 100u16..=599 {
        let Ok(code) = http::StatusCode::from_u16(code_num) else {
            continue;
        };
        let Some(reason) = code.canonical_reason() else {
            continue;
        };

        let needle = format!("{} {}", code.as_str(), reason);
        if text.contains(&needle) {
            if code.is_redirection()
                || code.is_server_error()
                || code == http::StatusCode::NOT_FOUND
                || code == http::StatusCode::REQUEST_TIMEOUT
                || code == http::StatusCode::CONFLICT
                || code == http::StatusCode::GONE
                || code == http::StatusCode::PRECONDITION_FAILED
                || code == http::StatusCode::RANGE_NOT_SATISFIABLE
                || code == http::StatusCode::EXPECTATION_FAILED
                || code == http::StatusCode::MISDIRECTED_REQUEST
                || code == http::StatusCode::UNPROCESSABLE_ENTITY
                || code == http::StatusCode::LOCKED
                || code == http::StatusCode::FAILED_DEPENDENCY
                || code == http::StatusCode::TOO_EARLY
                || code == http::StatusCode::UPGRADE_REQUIRED
                || code == http::StatusCode::PRECONDITION_REQUIRED
                || code == http::StatusCode::TOO_MANY_REQUESTS
                || code == http::StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS
            {
                return PublishError::Retryable(code);
            }
            return PublishError::Fatal(code);
        }
    }

    PublishError::Unknown
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq as sim_assert_eq;

    #[test]
    fn classifies_retryable_fatal_and_unknown_publish_errors() {
        let cases = [
            (
                "the remote server responded with 429 Too Many Requests",
                super::PublishError::Retryable(http::StatusCode::TOO_MANY_REQUESTS),
            ),
            (
                "the remote server responded with 500 Internal Server Error",
                super::PublishError::Retryable(http::StatusCode::INTERNAL_SERVER_ERROR),
            ),
            (
                "the remote server responded with 403 Forbidden",
                super::PublishError::Fatal(http::StatusCode::FORBIDDEN),
            ),
            (
                "the remote server responded with an unknown error",
                super::PublishError::Unknown,
            ),
        ];

        for (message, expected) in cases {
            let error = super::classify_publish_error(message);
            sim_assert_eq!(error, expected);
            sim_assert_eq!(error.code(), expected.code());
        }
    }

    /// Recognizes only Cargo's min-publish-age rejection, quoted from cargo 1.100.
    #[test]
    fn detects_dependencies_rejected_by_min_publish_age() {
        let too_new = r#"error: failed to prepare local package for uploading

Caused by:
  failed to select a version for the requirement `micromux = "=0.4.6"`
    version 0.4.6 is too new (published 35 seconds ago, minimum age 1 week)
  location searched: crates.io index
  required by package `micromux-control v0.4.6 (/repo/crates/micromux-control)`
  help: to preserve the min-publish-age, downgrade the requirement to "0.4.2"
  help: to use too-new packages anyways, re-resolve with `CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow`
"#;
        // A dry run warns about an existing version before the resolution error that fails it.
        let dry_run = format!(
            "warning: crate micromux-control@0.4.6 already exists on crates.io index\n{too_new}"
        );
        // A version the index does not list yet is the lag that publish_delay waits out.
        let missing_version = r#"error: failed to select a version for the requirement `micromux = "=0.4.7"`
candidate versions found which didn't match: 0.4.6, 0.4.5, 0.4.4, ...
location searched: crates.io index
required by package `micromux-control v0.4.7 (/repo/crates/micromux-control)`
"#;
        // A feature conflict can also come from index lag, so it must stay retryable.
        let feature_conflict = r"error: failed to select a version for `micromux`.
    ... required by package `micromux-control v0.4.7 (/repo/crates/micromux-control)`
versions that meet the requirements `^0.4` are: 0.4.6
the package `micromux-control` depends on `micromux`, with features: `control` but `micromux` does not have these features.
failed to select a version for `micromux` which could resolve this conflict
";

        // A caret requirement can match several releases that are all too new.
        let several = r#"error: failed to select a version for the requirement `micromux = "^0.4"`
  version 0.4.5 is too new (published 2 days ago, minimum age 1 week)
  version 0.4.6 is too new (published 35 seconds ago, minimum age 1 week)
location searched: crates.io index
"#;

        // Rejection names the package and every rejected release
        let micromux = Some(super::TooNewDependency {
            package: "micromux",
            versions: vec!["0.4.6"],
        });
        sim_assert_eq!(super::too_new_dependency(too_new), micromux);
        sim_assert_eq!(super::too_new_dependency(&dry_run), micromux);
        sim_assert_eq!(
            super::too_new_dependency(several),
            Some(super::TooNewDependency {
                package: "micromux",
                versions: vec!["0.4.5", "0.4.6"],
            })
        );

        // Index lag stays retryable
        sim_assert_eq!(super::too_new_dependency(missing_version), None);
        sim_assert_eq!(super::too_new_dependency(feature_conflict), None);
    }

    /// Treats only Cargo's duplicate error as published, not the warning a dry run prints.
    #[test]
    fn detects_already_published_versions() {
        let error = "    Updating crates.io index\n\
                     error: crate micromux@0.4.6 already exists on crates.io index\n";
        let dry_run_warning = "warning: crate micromux@0.4.6 already exists on crates.io index\n\
                               error: could not compile `micromux` (lib) due to 1 previous error\n";

        // Forced progress output loses its carriage return when terminal escapes are stripped.
        let after_progress = "    Updating crates.io index    error: crate micromux@0.4.6 \
                              already exists on crates.io index\n";

        assert!(super::is_already_published(error));
        assert!(super::is_already_published(after_progress));
        assert!(!super::is_already_published(dry_run_warning));
    }
}
