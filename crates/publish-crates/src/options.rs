//! Options that select workspace packages and configure publishing.

use color_eyre::eyre;
use std::path::PathBuf;
use tokio::time::Duration;

/// Configures workspace package selection and publishing behavior.
#[derive(Debug)]
pub struct Options {
    /// Path to a package or workspace directory, or directly to its `Cargo.toml`.
    pub path: PathBuf,

    /// Registry token passed to Cargo as `CARGO_REGISTRY_TOKEN`.
    ///
    /// A value of [`None`] leaves Cargo's existing credentials and environment unchanged.
    pub registry_token: Option<String>,

    /// Runs Cargo's publishing checks without uploading packages.
    ///
    /// Manifests are not modified in this mode. When [`Self::resolve_versions`] is enabled for a
    /// package with local dependencies, Cargo's dry-run is skipped because the resolved versions
    /// exist only in memory.
    pub dry_run: bool,

    /// Delay after a package becomes available before publishing its dependants.
    ///
    /// A value of [`None`] uses 30 seconds. The delay is not applied during a dry-run.
    pub publish_delay: Option<Duration>,

    /// Passes `--no-verify` to `cargo publish`.
    pub no_verify: bool,

    /// Replaces local path dependency requirements with exact workspace package versions.
    ///
    /// A local dependency such as `{ path = "../some/path" }` receives the version of the package
    /// at that path. Existing version requirements are also replaced.
    ///
    /// Versionless development dependencies on packages with `publish = false` remain path-only,
    /// so Cargo omits them from the published manifest. Other private local dependencies prevent
    /// publication.
    ///
    /// Unless [`Self::dry_run`] is enabled, this updates affected Cargo manifests.
    pub resolve_versions: bool,

    /// Workspace package names eligible for publishing.
    ///
    /// [`None`] or an empty list includes every publishable workspace package.
    pub include: Option<Vec<String>>,

    /// Workspace package names excluded from publishing.
    ///
    /// Exclusion takes precedence over [`Self::include`].
    pub exclude: Option<Vec<String>>,

    /// Maximum retries after the initial attempt for an intermittent publishing error.
    ///
    /// [`None`] uses twice the number of discovered workspace packages.
    pub max_retries: Option<usize>,

    /// Maximum number of packages to publish concurrently.
    ///
    /// [`None`] uses four. A value of zero is invalid.
    pub concurrency_limit: Option<usize>,

    /// Additional arguments passed to every `cargo publish` invocation.
    ///
    /// Each element is passed as one argument, without shell interpretation.
    pub extra_args: Vec<String>,
}

impl Options {
    pub(crate) fn validate(&self) -> eyre::Result<()> {
        if self.concurrency_limit == Some(0) {
            eyre::bail!("concurrency limit must be greater than zero");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use similar_asserts::assert_eq as sim_assert_eq;

    use crate::test_support::options;

    #[test]
    fn rejects_zero_concurrency_limit() {
        let mut options = options(PathBuf::from("Cargo.toml"));
        options.concurrency_limit = Some(0);

        let error = options
            .validate()
            .expect_err("zero concurrency must be rejected");

        sim_assert_eq!(
            error.to_string(),
            "concurrency limit must be greater than zero"
        );
    }
}
