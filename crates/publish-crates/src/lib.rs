use action_core as action;
use cargo_metadata::DependencyKind;
use color_eyre::{Section, eyre};
use futures::Future;
use futures::stream::{self, FuturesUnordered, StreamExt};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, Instant, interval, sleep};

const DATETIME_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[hour]:[minute]:[second]");

/// Options for publishing packages.
#[derive(Debug)]
pub struct Options {
    /// Path to package or workspace
    pub path: PathBuf,

    /// Cargo registry token
    pub registry_token: Option<String>,

    /// Perform dry-run
    /// This will perform all checks without publishing the package
    pub dry_run: bool,

    /// Delay before attempting to publish dependent crate
    pub publish_delay: Option<Duration>,

    /// Disable pre-publish validation checks
    pub no_verify: bool,

    /// Resolve missing versions for local packages.
    ///
    /// Versions of local packages that use `{ path = "../some/path" }`
    /// will be resolved to the version of the package the `path` is pointing to.
    /// Note that even if `version` is present, the resolved value will be used.
    ///
    /// **Note**: This will update your `Cargo.toml` manifest with the resolved version.
    pub resolve_versions: bool,

    /// Packages that should be published
    ///
    /// If using explicit include, specify all package names you wish to publish
    pub include: Option<Vec<String>>,

    /// Packages that should not be published
    ///
    /// Excluded package names have precedence over included package names.
    pub exclude: Option<Vec<String>>,

    /// Maximum number of retries when encountering intermittent errors.
    ///
    /// Common intermittent failures are:
    /// - 500 Internal Server Error
    /// - 429 Too Many Requests
    pub max_retries: Option<usize>,

    /// Maximum number of packages to publish concurrently.
    pub concurrency_limit: Option<usize>,
}

/// A cargo package.
struct Package {
    inner: cargo_metadata::Package,
    path: PathBuf,
    should_publish: bool,
    published: Mutex<bool>,
    deps: RwLock<HashMap<String, Arc<Package>>>,
    dependants: RwLock<HashMap<String, Arc<Package>>>,
}

impl std::fmt::Debug for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl std::fmt::Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Package")
            .field("name", &self.inner.name)
            .field("version", &self.inner.version.to_string())
            .field("deps", &self.deps.read().keys().collect::<Vec<_>>())
            .field(
                "dependants",
                &self.dependants.read().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Package {
    /// Returns `true` if the package has been successfully published.
    pub fn published(&self) -> bool {
        *self.published.lock()
    }

    /// Checks if the package is ready for publishing.
    ///
    /// A package can be published if all its dependencies have been published.
    pub fn ready(&self) -> bool {
        self.deps.read().values().all(|d| d.published())
    }

    /// Wait until the published package is available on the registry.
    pub async fn is_available(&self) -> eyre::Result<bool> {
        use crates_io_api::{AsyncClient, Error as RegistryError};
        use semver::Version;

        let api = AsyncClient::new(
            "publish_crates (https://github.com/romnn/publish-crates)",
            std::time::Duration::from_millis(1000),
        )?;

        let info = match api.get_crate(&self.inner.name).await {
            Ok(info) => info,
            Err(RegistryError::NotFound(_)) => return Ok(false),
            Err(err) => return Err(err.into()),
        };

        let mut versions = info
            .versions
            .iter()
            .filter_map(|v| match Version::parse(&v.num) {
                Ok(version) => Some((version, v)),
                Err(_) => None,
            });
        let Some((_, version)) = versions.find(|(ver, _)| ver == &self.inner.version) else {
            return Ok(false);
        };

        let client = reqwest::Client::new();
        let dl_response = client
            .head(format!("https://crates.io{}", version.dl_path))
            .send()
            .await?;
        Ok(dl_response.status() == reqwest::StatusCode::OK)
    }

    /// Wait until the published package is available on the registry.
    pub async fn wait_package_available(
        &self,
        timeout: impl Into<Option<Duration>>,
    ) -> eyre::Result<()> {
        let timeout = timeout
            .into()
            .unwrap_or_else(|| Duration::from_secs(2 * 60));
        let start = Instant::now();
        let mut ticker = interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            action::info!(
                "[{}@{}] checking if available",
                self.inner.name,
                self.inner.version,
            );
            if self.is_available().await? {
                return Ok(());
            }
            // check for timeout
            if Instant::now().duration_since(start) > timeout {
                eyre::bail!(
                    "exceeded timeout of {:?} waiting for crate {} {} to be published",
                    timeout,
                    self.inner.name,
                    self.inner.version.to_string()
                );
            }
        }
    }

    pub async fn attempt_publish(
        &self,
        mut cmd: async_process::Command,
        max_retries: usize,
    ) -> eyre::Result<()> {
        let mut attempt = 0;
        loop {
            attempt += 1;

            if attempt > 1 {
                action::warning!(
                    "[{}@{}] publishing (attempt {}/{})",
                    self.inner.name,
                    self.inner.version,
                    attempt,
                    max_retries
                );
            }

            let output = cmd.output().await?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            action::debug!("{}", &stdout);
            action::debug!("{}", &stderr);

            if output.status.success() {
                return Ok(());
            }
            action::warning!("{}", &stdout);
            action::warning!("{}", &stderr);

            // Treat manifest verification failures as fatal so we don't retry
            // for hours on static configuration problems.
            if stderr
                .contains("all dependencies must have a version requirement specified when publishing.")
            {
                eyre::bail!(
                    "command {:?} failed due to manifest verification error: {}",
                    cmd,
                    stderr
                );
            }

            if stderr.contains("already exists on crates.io index") {
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
                            // 10 minutes
                            std::time::Duration::from_secs(10 * 60)
                        }
                        // 5 minutes
                        _ => std::time::Duration::from_secs(5 * 60),
                    }
                }
                PublishError::Unknown => {
                    action::warning!(
                        "[{}@{}] unknown failure",
                        self.inner.name,
                        self.inner.version,
                    );
                    // 5 minutes
                    std::time::Duration::from_secs(5 * 60)
                }
            };

            if attempt >= max_retries {
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

    /// Publishes this package
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
            // skip checking if local package versions are available on crates.io as they are not
            // published during dry-run
            if options.resolve_versions && !self.deps.read().is_empty() {
                // cmd.arg("--offline");
                // skip cargo dry-run as it will always fail
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
            // when resolving versions, we may write to Cargo.toml
            cmd.arg("--allow-dirty");
        }

        let max_retries = options.max_retries.unwrap_or(10);
        self.attempt_publish(cmd, max_retries).await?;

        if options.dry_run {
            action::info!(
                "[{}@{}] dry-run: skip waiting for successful publish",
                &self.inner.name,
                self.inner.version
            );
            *self.published.lock() = true;
            return Ok(self);
        }

        // wait for package to be available on the registry
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

type TaskFut = dyn Future<Output = eyre::Result<Arc<Package>>>;

fn find_packages(
    metadata: &cargo_metadata::Metadata,
    options: &Options,
) -> impl Iterator<Item = (PathBuf, Arc<Package>)> {
    let packages = metadata.workspace_packages();
    packages.into_iter().filter_map(move |package| {
        let should_publish = package.publish.as_ref().is_none_or(|p| !p.is_empty());

        let is_included = options
            .include
            .as_ref()
            .is_none_or(|inc| inc.is_empty() || inc.contains(&package.name));

        let is_excluded = options
            .exclude
            .as_ref()
            .is_some_and(|excl| excl.contains(&package.name));

        let should_publish = should_publish && is_included && !is_excluded;

        let path: PathBuf = package.manifest_path.parent()?.into();
        Some((
            path.clone(),
            Arc::new(Package {
                inner: package.clone(),
                path,
                should_publish,
                published: Mutex::new(false),
                deps: RwLock::new(HashMap::new()),
                dependants: RwLock::new(HashMap::new()),
            }),
        ))
    })
}

async fn build_dag(
    packages: &HashMap<PathBuf, Arc<Package>>,
    options: &Options,
) -> eyre::Result<()> {
    let packages_iter = packages.values().filter(|p| p.should_publish);
    let results: Vec<_> = stream::iter(packages_iter)
        .map(|p| async move {
            use toml_edit::{value, DocumentMut};
            let manifest_path = &p.inner.manifest_path;
            let manifest = tokio::fs::read_to_string(manifest_path).await?;
            let mut manifest = manifest.parse::<DocumentMut>()?;
            let mut need_update = false;

            for dep in &p.inner.dependencies {
                let mut dep_version = dep.req.clone();
                if let Some(path) = dep.path.as_ref().map(PathBuf::from) {
                    // also if the version is set, we want to resolve automatically?
                    // OR we allow changing and always set allow-dirty
                    // dep_version == semver::VersionReq::STAR &&
                    let resolved = packages.get(&path).ok_or(eyre::eyre!(
                        "{}: could not resolve local dependency {}",
                        &p.inner.name,
                        path.display()
                    ))?;

                    // ensure that all local dependencies for a package
                    // that should be published are also going to
                    // be published
                    if !resolved.should_publish {
                        eyre::bail!(
                            "{}: cannot publish because dependency {} will not be published",
                            &p.inner.name,
                            &dep.name,
                        );
                    }

                    if options.resolve_versions {
                        // use version from the manifest the path points to
                        dep_version = semver::VersionReq {
                            comparators: vec![semver::Comparator {
                                op: semver::Op::Exact,
                                major: resolved.inner.version.major,
                                minor: Some(resolved.inner.version.minor),
                                patch: Some(resolved.inner.version.patch),
                                pre: semver::Prerelease::EMPTY,
                            }],
                        };

                        let changed = dep_version != dep.req;
                        if changed {
                            // update cargo manifest
                            let section = match dep.kind {
                                DependencyKind::Normal => Some("dependencies"),
                                DependencyKind::Development => Some("dev-dependencies"),
                                DependencyKind::Build => Some("build-dependencies"),
                                _ => None,
                            };
                            if let Some(section) = section {
                                manifest[section][&dep.name]["version"] =
                                    value(dep_version.to_string());
                                manifest[section][&dep.name]
                                    .as_inline_table_mut()
                                    .map(toml_edit::InlineTable::fmt);
                                need_update = true;
                            }
                        }
                    }

                    p.deps
                        .write()
                        .insert(resolved.inner.name.to_string(), resolved.clone());

                    resolved
                        .dependants
                        .write()
                        .insert(p.inner.name.to_string(), p.clone());
                }

                let is_dev_dep = dep.kind == DependencyKind::Development;
                let is_non_local_dep = !is_dev_dep || dep.path.is_none();
                let is_missing_exact_version = dep_version == semver::VersionReq::STAR;

                if is_missing_exact_version && is_non_local_dep {
                    return Err(eyre::eyre!(
                        "{}: dependency {} has no specific version ({})",
                        &p.inner.name,
                        &dep.name,
                        dep_version
                    ).suggestion("to automatically resolve versions of local workspace members, use '--resolve-versions'"));
                }
            }

            // write updated cargo manifest
            if !options.dry_run && need_update {
                use tokio::io::AsyncWriteExt;
                action::debug!("{}", &manifest.to_string());
                action::info!("[{}@{}] updating {}", p.inner.name, p.inner.version, p.inner.manifest_path);
                let mut f = tokio::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&p.inner.manifest_path)
                    .await?;
                f.write_all(manifest.to_string().as_bytes()).await?;
            }

            Ok(())
        })
        .buffer_unordered(8)
        .collect()
        .await;

    // fail on error
    results.into_iter().collect::<eyre::Result<Vec<_>>>()?;
    Ok(())
}

/// Update versions in `[workspace.dependencies]` for local path dependencies
/// when `--resolve-versions` is enabled.
///
/// This is required for newer Cargo versions which enforce that any
/// published dependency that has a `path` also specifies an explicit
/// version requirement. For workspaces that rely on `[workspace.dependencies]`
/// plus `foo.workspace = true` in member manifests, the version must be set
/// on the workspace-level dependency.
async fn update_workspace_dependencies(
    metadata: &cargo_metadata::Metadata,
    packages: &HashMap<PathBuf, Arc<Package>>,
    options: &Options,
) -> eyre::Result<()> {
    use toml_edit::{value, DocumentMut, Value};

    // Fast path: nothing to do when resolve_versions is disabled.
    if !options.resolve_versions {
        return Ok(());
    }

    let workspace_manifest_path = metadata.workspace_root.join("Cargo.toml");
    let manifest = tokio::fs::read_to_string(&workspace_manifest_path).await?;
    let mut manifest = manifest.parse::<DocumentMut>()?;
    let mut need_update = false;

    // Build an index from crate name to package for quick lookup.
    let mut name_to_pkg: HashMap<String, &Arc<Package>> = HashMap::new();
    for pkg in packages.values() {
        name_to_pkg.insert(pkg.inner.name.to_string(), pkg);
    }

    if let Some(workspace) = manifest.get_mut("workspace")
        && let Some(deps_item) = workspace.get_mut("dependencies")
            && let Some(table) = deps_item.as_table_mut() {
                for (name, item) in table.iter_mut() {
                    // Only consider workspace dependencies that correspond
                    // to local workspace members.
                    let dep_name = name.get().to_string();
                    let Some(pkg) = name_to_pkg.get(&dep_name) else {
                        continue;
                    };

                    // Skip simple string dependencies like `foo = "1"`.
                    if item.is_str() {
                        continue;
                    }

                    // Handle both inline tables and normal tables.
                    if let Some(inline) = item.as_inline_table_mut() {
                        let has_path = inline.get("path").is_some();
                        let has_version = inline.get("version").is_some();

                        if !has_path || has_version {
                            continue;
                        }

                        let ver_req = format!("={}", pkg.inner.version);
                        inline.insert("version", Value::from(ver_req));
                        inline.fmt();
                        need_update = true;
                    } else if let Some(table_item) = item.as_table_mut() {
                        let has_path = table_item.get("path").is_some();
                        let has_version = table_item.get("version").is_some();

                        if !has_path || has_version {
                            continue;
                        }

                        let ver_req = format!("={}", pkg.inner.version);
                        table_item["version"] = value(ver_req);
                        need_update = true;
                    }
                }
            }

    // Persist changes to the workspace manifest.
    if !options.dry_run && need_update {
        use tokio::io::AsyncWriteExt;
        action::debug!("{}", &manifest.to_string());
        action::info!(
            "updating workspace manifest {}",
            workspace_manifest_path.as_str()
        );
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&workspace_manifest_path)
            .await?;
        f.write_all(manifest.to_string().as_bytes()).await?;
    }

    Ok(())
}

/// Publishes packages of a project on crates.io.
///
/// # Errors
/// If any package cannot be published.
pub async fn publish(mut options: Options) -> eyre::Result<()> {
    action::info!("searching cargo packages at {}", options.path.display());

    let manifest_path = if options.path.is_file() {
        options.path.clone()
    } else {
        options.path.join("Cargo.toml")
    };
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&manifest_path)
        .exec()?;

    let packages: HashMap<PathBuf, Arc<Package>> = find_packages(&metadata, &options).collect();
    // For workspaces using `[workspace.dependencies]`, ensure local path
    // dependencies have explicit versions before we start publishing.
    update_workspace_dependencies(&metadata, &packages, &options).await?;
    build_dag(&packages, &options).await?;

    action::info!(
        "found packages: {:?}",
        packages
            .values()
            .map(|p| p.inner.name.clone())
            .collect::<Vec<_>>()
    );

    options.max_retries = Some(options.max_retries.unwrap_or(2 * packages.len()));
    let options = Arc::new(options);

    if packages.is_empty() {
        // fast path: nothing to do here
        return Ok(());
    }
    let mut ready: VecDeque<Arc<Package>> =
        packages.values().filter(|p| p.ready()).cloned().collect();

    let mut tasks: FuturesUnordered<Pin<Box<TaskFut>>> = FuturesUnordered::new();

    let limit = options.concurrency_limit.unwrap_or(4);
    let limit = Arc::new(Semaphore::new(limit));

    loop {
        // check if we are done
        if tasks.is_empty() && ready.is_empty() {
            break;
        }

        // start running ready tasks
        loop {
            // acquire permit
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                break;
            };
            // check if we can publish
            match ready.pop_front() {
                Some(p) if !p.should_publish => {
                    action::info!(
                        "[{}@{}] skipping (publish=false)",
                        p.inner.name,
                        p.inner.version
                    );
                }
                Some(p) => {
                    tasks.push({
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            let res = p.publish(options).await;

                            // release permit
                            drop(permit);
                            res
                        })
                    });
                }
                // no more tasks
                None => break,
            }
        }

        // wait for a task to complete
        match tasks.next().await {
            Some(Err(err)) => {
                eyre::bail!("a task failed: {}", err)
            }
            Some(Ok(completed)) => {
                // update ready tasks
                ready.extend(
                    completed
                        .dependants
                        .read()
                        .values()
                        .filter(|d| d.ready() && !d.published())
                        .cloned(),
                );
            }
            None => {}
        }
    }

    if !packages
        .values()
        .all(|p| !p.should_publish || p.published())
    {
        eyre::bail!("not all published");
    }

    Ok(())
}

/// Classification of publishing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PublishError {
    Unknown,
    Retryable(http::StatusCode),
    Fatal(http::StatusCode),
}

impl PublishError {
    #[must_use]
    pub fn code(&self) -> Option<&http::StatusCode> {
        match self {
            Self::Unknown => None,
            Self::Retryable(code) | Self::Fatal(code) => Some(code),
        }
    }
}

/// Classify publish errors based on the error message.
///
/// This approach assumes that the error messages of `cargo publish` include network errors
/// in the form `<code> <canonical_reason>`.
///
/// # Returns:
/// - `Retryable(code)` if a temporary / intermittent error was detected
/// - `Fatal(code)` if a fatal network error was detected (such as missing permissions)
/// - `Unknown` otherwise
fn classify_publish_error(text: &str) -> PublishError {
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
    fn classify_publish_error() {
        sim_assert_eq!(
            super::classify_publish_error(
                "the remote server responded with an error (status 429 Too Many Requests): You have published too many new crates in a short period of time. Please try again after Mon, 21 Apr 2025 19:31:32 GMT or email help@crates.io to have your limit increased."
            ),
            super::PublishError::Retryable(http::StatusCode::TOO_MANY_REQUESTS)
        );

        sim_assert_eq!(
            super::classify_publish_error(
                "the remote server responded with an error (status 500 Internal Server Error): Internal Server Error"
            ),
            super::PublishError::Retryable(http::StatusCode::INTERNAL_SERVER_ERROR)
        );

        sim_assert_eq!(
            super::classify_publish_error(
                "the remote server responded with some error we don't know more about"
            ),
            super::PublishError::Unknown,
        );
    }

    #[tokio::test]
    async fn update_workspace_dependencies_adds_versions_for_local_path_deps() {
        use std::path::PathBuf;

        // Synthetic workspace manifest with a path-only workspace dependency
        // and a normal crates.io dependency that should be left untouched.
        let workspace_manifest = r#"
[workspace]
members = ["crates/foo", "crates/bar"]

[workspace.package]
version = "1.2.3"

[workspace.dependencies]
foo = { path = "crates/foo" }
serde = "1"

[workspace.dependencies.bar]
path = "crates/bar"
"#;

        // Create a temporary workspace directory with Cargo.toml.
        let tmp = tempfile::tempdir().unwrap();
        let workspace_manifest_path = tmp.path().join("Cargo.toml");
        std::fs::write(&workspace_manifest_path, workspace_manifest).unwrap();

        // Minimal dummy member manifest so that cargo_metadata can see the package.
        let crates_dir = tmp.path().join("crates");
        std::fs::create_dir_all(&crates_dir).unwrap();

        // Member `foo` for the inline table workspace dependency.
        let foo_dir = crates_dir.join("foo");
        std::fs::create_dir_all(foo_dir.join("src")).unwrap();
        std::fs::write(
            foo_dir.join("Cargo.toml"),
            r#"[package]
name = "foo"
version = "1.2.3"
edition = "2021"
"#,
        )
        .unwrap();
        // Add a minimal lib target so Cargo considers this crate valid.
        std::fs::write(foo_dir.join("src/lib.rs"), "pub fn _dummy() {}\n").unwrap();

        // Member `bar` for the table-style workspace dependency.
        let bar_dir = crates_dir.join("bar");
        std::fs::create_dir_all(bar_dir.join("src")).unwrap();
        std::fs::write(
            bar_dir.join("Cargo.toml"),
            r#"[package]
name = "bar"
version = "1.2.3"
edition = "2021"
"#,
        )
        .unwrap();
        // Add a minimal lib target so Cargo considers this crate valid.
        std::fs::write(bar_dir.join("src/lib.rs"), "pub fn _dummy() {}\n").unwrap();

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .unwrap();

        // Build a packages map compatible with update_workspace_dependencies.
        let mut packages = std::collections::HashMap::<PathBuf, std::sync::Arc<super::Package>>::new();
        for pkg in metadata.workspace_packages() {
            let path: PathBuf = pkg.manifest_path.parent().unwrap().into();
            packages.insert(
                path.clone(),
                std::sync::Arc::new(super::Package {
                    inner: pkg.clone(),
                    path,
                    should_publish: true,
                    published: parking_lot::Mutex::new(false),
                    deps: parking_lot::RwLock::new(std::collections::HashMap::new()),
                    dependants: parking_lot::RwLock::new(std::collections::HashMap::new()),
                }),
            );
        }

        let mut options = super::Options {
            path: workspace_manifest_path.clone(),
            registry_token: None,
            dry_run: false,
            publish_delay: None,
            no_verify: false,
            resolve_versions: true,
            include: None,
            exclude: None,
            max_retries: None,
            concurrency_limit: None,
        };

        // Exercise the helper with resolve_versions enabled; this should not error.
        super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .unwrap();

        // Exercise the helper again with resolve_versions disabled; this should
        // also be a no-op without errors. We deliberately avoid asserting on
        // exact file contents here to keep the test robust across platforms
        // and toml_edit formatting differences.
        options.resolve_versions = false;
        super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .unwrap();
    }
}
