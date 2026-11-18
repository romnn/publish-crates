//! Running `cargo publish` for one package, with retries and the `min-publish-age` exemption.

use crate::Options;
use crate::diagnostics::{
    INCOMPATIBLE_PUBLISH_AGE_ENV, PublishError, TooNewDependency, classify_publish_error,
    is_already_published, too_new_dependency,
};
use crate::package::Package;
use action_core as action;
use color_eyre::eyre;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

const DATETIME_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[hour]:[minute]:[second]");

impl Package {
    /// Decides whether Cargo may publish this package with too new dependencies allowed.
    ///
    /// A local dependency was published moments ago, so only waiting out the age policy would
    /// satisfy it.
    /// Cargo has no per-package exemption, so this allows fresh versions for the package's whole
    /// invocation, and only once Cargo has rejected the exact version of a local dependency.
    /// Crate names are unique on the registry, so that version is the local package's own release.
    fn exempts_too_new_dependency(&self, rejected: &TooNewDependency<'_>) -> bool {
        let is_local_release = self.deps.read().get(rejected.package).is_some_and(|local| {
            rejected.versions.iter().any(|version| {
                version
                    .parse::<semver::Version>()
                    .is_ok_and(|version| version == local.inner.version)
            })
        });
        if !is_local_release {
            return false;
        }
        action::warning!(
            "[{}@{}] local dependency {} is newer than the registry's min-publish-age; \
             publishing again with {INCOMPATIBLE_PUBLISH_AGE_ENV}=allow",
            self.inner.name,
            self.inner.version,
            rejected.package,
        );
        true
    }

    /// Runs `cmd` until it publishes the package, retrying intermittent registry failures.
    ///
    /// Unless `caller_chose_publish_age` is set, a local dependency that the registry's
    /// `min-publish-age` rejects is allowed for one more attempt that does not count as a retry.
    pub async fn attempt_publish(
        &self,
        mut cmd: async_process::Command,
        max_retries: usize,
        caller_chose_publish_age: bool,
    ) -> eyre::Result<()> {
        let mut attempt = 0;
        let mut allows_too_new_dependencies = false;
        loop {
            attempt += 1;

            if attempt > 1 {
                action::warning!(
                    "[{}@{}] publishing (attempt {}/{})",
                    self.inner.name,
                    self.inner.version,
                    attempt,
                    max_retries.saturating_add(1)
                );
            }

            let output = cmd.output().await?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            // The checks below match plain text, and arguments such as `--color always` style it.
            let stderr = strip_ansi_escapes::strip_str(String::from_utf8_lossy(&output.stderr));
            action::debug!("{}", &stdout);
            action::debug!("{}", &stderr);

            if output.status.success() {
                return Ok(());
            }
            action::warning!("{}", &stdout);
            action::warning!("{}", &stderr);

            // Treat manifest verification failures as fatal so we don't retry
            // for hours on static configuration problems.
            if stderr.contains(
                "all dependencies must have a version requirement specified when publishing.",
            ) {
                eyre::bail!(
                    "command {:?} failed due to manifest verification error: {}",
                    cmd,
                    stderr
                );
            }

            if let Some(rejected) = too_new_dependency(&stderr) {
                if !caller_chose_publish_age
                    && !allows_too_new_dependencies
                    && self.exempts_too_new_dependency(&rejected)
                {
                    cmd.env(INCOMPATIBLE_PUBLISH_AGE_ENV, "allow");
                    allows_too_new_dependencies = true;
                    // This repeats a deterministic failure with a fix, not an intermittent one.
                    attempt -= 1;
                    continue;
                }
                eyre::bail!(
                    "command {:?} failed because dependency {} is newer than the registry's \
                     min-publish-age, which retrying cannot change: {}",
                    cmd,
                    rejected.package,
                    stderr
                );
            }

            if is_already_published(&stderr) {
                return Ok(());
            }

            let error = classify_publish_error(&stderr);
            let wait_duration = match error {
                PublishError::Fatal(_) => {
                    eyre::bail!("command {:?} failed: {}", cmd, stderr);
                }
                PublishError::Retryable(code) => {
                    action::warning!(
                        "[{}@{}] intermittent failure: {} {}",
                        self.inner.name,
                        self.inner.version,
                        code.as_u16(),
                        code.canonical_reason().unwrap_or_default(),
                    );
                    match code {
                        http::StatusCode::TOO_MANY_REQUESTS => {
                            // Rate limits back off for ten minutes to respect registry throttling.
                            std::time::Duration::from_mins(10)
                        }
                        // Other retryable failures use a five-minute backoff.
                        _ => std::time::Duration::from_mins(5),
                    }
                }
                PublishError::Unknown => {
                    action::warning!(
                        "[{}@{}] unknown failure",
                        self.inner.name,
                        self.inner.version,
                    );
                    // Unknown failures use a five-minute backoff before another attempt.
                    std::time::Duration::from_mins(5)
                }
            };

            if attempt > max_retries {
                eyre::bail!("command {:?} failed: {}", cmd, stderr);
            }

            let next_attempt = std::time::SystemTime::now() + wait_duration;
            action::warning!(
                "[{}@{}] attempting again in {wait_duration:?} at {}",
                self.inner.name,
                self.inner.version,
                time::OffsetDateTime::from(next_attempt)
                    .format(DATETIME_FORMAT)
                    .unwrap_or_else(|_| humantime::format_rfc3339(next_attempt).to_string())
            );
            sleep(wait_duration).await;
        }
    }

    /// Publishes this package after all local dependencies are available.
    pub async fn publish(self: Arc<Self>, options: Arc<Options>) -> eyre::Result<Arc<Self>> {
        use async_process::Command;

        action::info!("[{}@{}] publishing", self.inner.name, self.inner.version);

        let mut cmd = Command::new("cargo");
        cmd.arg("publish");

        if options.no_verify {
            cmd.arg("--no-verify");
        }
        cmd.current_dir(&self.path);
        if let Some(ref token) = options.registry_token {
            cmd.env("CARGO_REGISTRY_TOKEN", token);
        }
        if options.dry_run {
            cmd.arg("--dry-run");
            // Local package versions are unavailable on crates.io during a dry-run.
            if options.resolve_versions && !self.deps.read().is_empty() {
                // Skip Cargo's dry-run because its dependency lookup would always fail.
                action::info!(
                    "[{}@{}] dry-run: proceed without `cargo publish --dry-run` due to resolve version incompatibility",
                    self.inner.name,
                    self.inner.version
                );
                *self.published.lock() = true;
                return Ok(self);
            }
        }
        if options.resolve_versions {
            // Resolved versions intentionally modify Cargo.toml before publishing.
            cmd.arg("--allow-dirty");
        }
        cmd.args(&options.extra_args);

        let max_retries = options.max_retries.unwrap_or(10);
        let caller_chose_publish_age = std::env::var_os(INCOMPATIBLE_PUBLISH_AGE_ENV).is_some();
        self.attempt_publish(cmd, max_retries, caller_chose_publish_age)
            .await?;

        if options.dry_run {
            action::info!(
                "[{}@{}] dry-run: skip waiting for successful publish",
                &self.inner.name,
                self.inner.version
            );
            *self.published.lock() = true;
            return Ok(self);
        }

        // Dependants can publish only after the registry serves this exact version.
        self.wait_package_available(None).await?;

        let publish_delay = options
            .publish_delay
            .unwrap_or_else(|| Duration::from_secs(30));
        sleep(publish_delay).await;

        let mut cmd = Command::new("cargo");
        cmd.arg("update");
        cmd.current_dir(&self.path);
        let output = cmd.output().await?;
        if !output.status.success() {
            eyre::bail!("command {:?} failed", cmd);
        }

        *self.published.lock() = true;
        action::info!(
            "[{}@{}] published successfully",
            self.inner.name,
            self.inner.version
        );

        Ok(self)
    }
}

// Every test drives `attempt_publish` with `sh` standing in for `cargo`.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::path::Path;

    use similar_asserts::assert_eq as sim_assert_eq;

    use crate::test_support::consumer_of_local_foo;

    /// Returns Cargo's rejection of `package@version` under `min-publish-age`.
    fn too_new_stderr(package: &str, version: &str) -> String {
        format!(
            "error: failed to select a version for the requirement `{package} = \"={version}\"`\n  \
             version {version} is too new (published 3 seconds ago, minimum age 1 week)"
        )
    }

    /// Returns a stand-in for `cargo publish` that records each run's age policy in `log`.
    ///
    /// Under Cargo's default policy it fails with `strict_stderr`.
    /// With too new dependencies allowed it fails with `allowed_stderr`, or succeeds without one.
    /// The caller's own policy is removed so that only the publisher's override reaches it.
    fn publish_stub(
        log: &Path,
        strict_stderr: &str,
        allowed_stderr: Option<&str>,
    ) -> async_process::Command {
        let mut cmd = async_process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                r#"if [ "${var}" = allow ]; then
  echo allow >> "$LOG"
  [ -z "$ALLOWED_STDERR" ] && exit 0
  printf '%s\n' "$ALLOWED_STDERR" >&2
else
  echo strict >> "$LOG"
  printf '%s\n' "$STRICT_STDERR" >&2
fi
exit 1"#,
                var = crate::diagnostics::INCOMPATIBLE_PUBLISH_AGE_ENV,
            ))
            .env_remove(crate::diagnostics::INCOMPATIBLE_PUBLISH_AGE_ENV)
            .env("LOG", log)
            .env("STRICT_STDERR", strict_stderr)
            .env("ALLOWED_STDERR", allowed_stderr.unwrap_or_default());
        cmd
    }

    /// Runs `attempt_publish`, bounded so that any retry delay fails the test.
    async fn attempt_bounded(
        package: &crate::package::Package,
        cmd: async_process::Command,
        max_retries: usize,
        caller_chose_publish_age: bool,
    ) -> color_eyre::eyre::Result<()> {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            package.attempt_publish(cmd, max_retries, caller_chose_publish_age),
        )
        .await
        .expect("a min-publish-age rejection must not wait for a retry")
    }

    /// Returns each line the stand-in recorded in `log`, in order.
    fn recorded(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Allows a freshly published local dependency on one more run that spends no retry.
    #[tokio::test]
    async fn publishes_after_allowing_a_too_new_local_dependency() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let consumer = consumer_of_local_foo(temp.path()).await;
        let log = temp.path().join("runs.log");

        // No retry budget, so the allowed run must not count as a retry.
        let stub = publish_stub(&log, &too_new_stderr("foo", "1.2.3"), None);
        attempt_bounded(&consumer, stub, 0, false)
            .await
            .expect("publishing must succeed once the local dependency is allowed");

        // The default policy runs first, and the override only follows its rejection.
        sim_assert_eq!(recorded(&log), ["strict", "allow"]);
    }

    /// Keeps the age policy for rejections that are not this run's local releases.
    #[tokio::test]
    async fn fails_fast_when_the_too_new_release_is_not_local() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let consumer = consumer_of_local_foo(temp.path()).await;

        // A registry crate, an older release of the local package, and a caller's own policy
        for (case, stderr, caller_chose_publish_age) in [
            ("registry", too_new_stderr("serde", "1.0.228"), false),
            ("other release", too_new_stderr("foo", "1.2.2"), false),
            ("caller policy", too_new_stderr("foo", "1.2.3"), true),
        ] {
            let log = temp.path().join(format!("{case}.log"));
            // A retry budget, so falling through to the retry path would hit its delay.
            let stub = publish_stub(&log, &stderr, None);
            let error = attempt_bounded(&consumer, stub, 1, caller_chose_publish_age)
                .await
                .expect_err("a too new dependency that is not exempt must fail publishing");

            // Publishing stops after the first run and names the age policy.
            assert!(
                error.to_string().contains("min-publish-age"),
                "{case}: error must name the age policy: {error}"
            );
            sim_assert_eq!(recorded(&log), ["strict"], "{case}");
        }
    }

    /// Recognizes a published version in styled Cargo output instead of retrying it.
    #[tokio::test]
    async fn styled_duplicate_error_counts_as_published() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let consumer = consumer_of_local_foo(temp.path()).await;
        let log = temp.path().join("runs.log");

        // Cargo styles its errors when the caller forwards an argument such as `--color always`.
        let styled = "\u{1b}[1m\u{1b}[91merror\u{1b}[0m\u{1b}[1m:\u{1b}[0m crate \
                      consumer@1.2.3 already exists on crates.io index";
        let stub = publish_stub(&log, styled, None);

        // A retry budget, so a missed match would hit the retry delay.
        attempt_bounded(&consumer, stub, 1, false)
            .await
            .expect("an already published version must count as published");
        sim_assert_eq!(recorded(&log), ["strict"]);
    }

    /// Reports a dry run that fails after the override, despite its duplicate warning.
    #[tokio::test]
    async fn dry_run_failure_after_the_override_is_not_a_duplicate() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let consumer = consumer_of_local_foo(temp.path()).await;
        let log = temp.path().join("runs.log");

        // A dry run of an existing version warns about it on every run.
        let warning = "warning: crate consumer@1.2.3 already exists on crates.io index";
        let strict = format!("{warning}\n{}", too_new_stderr("foo", "1.2.3"));
        let allowed = format!("{warning}\nerror: could not compile `consumer`");
        let stub = publish_stub(&log, &strict, Some(&allowed));

        let error = attempt_bounded(&consumer, stub, 0, false)
            .await
            .expect_err("a dry run that fails to compile must fail publishing");

        // The second run's compile error is reported, not mistaken for a published version.
        assert!(
            error.to_string().contains("could not compile"),
            "error must report the compile failure: {error}"
        );
        sim_assert_eq!(recorded(&log), ["strict", "allow"]);
    }
}
