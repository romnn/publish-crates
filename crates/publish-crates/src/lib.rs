//! Publishes interdependent Cargo workspace packages in dependency order.
//!
//! The crate discovers workspace members with Cargo metadata, resolves local dependency order, and
//! runs `cargo publish` concurrently for packages whose dependencies are available.
//!
//! # Examples
//!
//! ```no_run
//! # // Uses `no_run` because publishing requires a Cargo workspace and registry access.
//! # async fn example() -> color_eyre::eyre::Result<()> {
//! use publish_crates::{Options, publish};
//! use std::path::PathBuf;
//!
//! publish(Options {
//!     path: PathBuf::from("Cargo.toml"),
//!     registry_token: None,
//!     dry_run: true,
//!     publish_delay: None,
//!     no_verify: false,
//!     resolve_versions: false,
//!     include: None,
//!     exclude: None,
//!     max_retries: None,
//!     concurrency_limit: Some(4),
//!     extra_args: Vec::new(),
//! })
//! .await?;
//! # Ok(())
//! # }
//! ```

use action_core as action;
use cargo_metadata::DependencyKind;
use cargo_metadata::cargo_platform::Platform;
use color_eyre::{Section, eyre};
use futures::Future;
use futures::stream::{self, FuturesUnordered, StreamExt};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, Instant, interval, sleep};

const DATETIME_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[hour]:[minute]:[second]");

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
    fn validate(&self) -> eyre::Result<()> {
        if self.concurrency_limit == Some(0) {
            eyre::bail!("concurrency limit must be greater than zero");
        }
        Ok(())
    }
}

/// Tracks publishing state and local dependency edges for a Cargo package.
struct Package {
    inner: cargo_metadata::Package,
    path: PathBuf,
    publishable: bool,
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

    /// Checks whether the package is ready for publishing.
    ///
    /// A package can be published if all its dependencies have been published.
    pub fn ready(&self) -> bool {
        self.deps.read().values().all(|d| d.published())
    }

    /// Checks whether this package version is downloadable from crates.io.
    pub async fn is_available(&self) -> eyre::Result<bool> {
        use crates_io_api::{AsyncClient, Error as RegistryError};
        use semver::Version;

        let api = AsyncClient::new(
            "publish_crates (https://github.com/romnn/publish-crates)",
            std::time::Duration::from_secs(1),
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

    /// Waits until the published package is available on the registry.
    pub async fn wait_package_available(
        &self,
        timeout: impl Into<Option<Duration>>,
    ) -> eyre::Result<()> {
        let timeout = timeout.into().unwrap_or_else(|| Duration::from_mins(2));
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
            // Check the timeout after every registry probe.
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
                    max_retries.saturating_add(1)
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
            if stderr.contains(
                "all dependencies must have a version requirement specified when publishing.",
            ) {
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

type TaskFut = dyn Future<Output = eyre::Result<Arc<Package>>>;

fn find_packages(
    metadata: &cargo_metadata::Metadata,
    options: &Options,
) -> impl Iterator<Item = (PathBuf, Arc<Package>)> {
    let packages = metadata.workspace_packages();
    packages.into_iter().filter_map(move |package| {
        let publishable = package.publish.as_ref().is_none_or(|p| !p.is_empty());

        let is_included = options
            .include
            .as_ref()
            .is_none_or(|inc| inc.is_empty() || inc.contains(&package.name));

        let is_excluded = options
            .exclude
            .as_ref()
            .is_some_and(|excl| excl.contains(&package.name));

        let should_publish = publishable && is_included && !is_excluded;

        let path: PathBuf = package.manifest_path.parent()?.into();
        Some((
            path.clone(),
            Arc::new(Package {
                inner: package.clone(),
                path,
                publishable,
                should_publish,
                published: Mutex::new(false),
                deps: RwLock::new(HashMap::new()),
                dependants: RwLock::new(HashMap::new()),
            }),
        ))
    })
}

fn update_dependency_version(
    manifest: &mut toml_edit::DocumentMut,
    package_name: &str,
    dependency: &cargo_metadata::Dependency,
    version: &semver::VersionReq,
) -> eyre::Result<bool> {
    use toml_edit::value;

    let section = dependency_section(dependency);
    let Some(section) = section else {
        return Ok(false);
    };

    let dependency_key = dependency.rename.as_deref().unwrap_or(&dependency.name);
    let manifest_dependency = manifest_dependency_mut(manifest, dependency).ok_or_else(|| {
        eyre::eyre!("{package_name}: dependency {dependency_key} is missing from {section}",)
    })?;
    {
        let dependency_table = manifest_dependency.as_table_like_mut().ok_or_else(|| {
            eyre::eyre!(
                "{package_name}: dependency {dependency_key} does not use a detailed manifest entry",
            )
        })?;
        dependency_table.insert("version", value(version.to_string()));
    }
    if let Some(inline_table) = manifest_dependency.as_inline_table_mut() {
        inline_table.fmt();
    }

    Ok(true)
}

fn dependency_section(dependency: &cargo_metadata::Dependency) -> Option<&'static str> {
    match dependency.kind {
        DependencyKind::Normal => Some("dependencies"),
        DependencyKind::Development => Some("dev-dependencies"),
        DependencyKind::Build => Some("build-dependencies"),
        _ => None,
    }
}

/// Finds the `[target.'...']` key that matches a dependency's target platform.
///
/// Keys are compared as parsed platforms because Cargo accepts arbitrary spacing inside
/// `cfg(...)` expressions, so the manifest text may differ from the canonical rendering.
fn target_table_key(manifest: &toml_edit::DocumentMut, target: &Platform) -> Option<String> {
    let targets = manifest.get("target")?.as_table_like()?;
    targets
        .iter()
        .map(|(key, _)| key)
        .find(|key| key.parse::<Platform>().ok().as_ref() == Some(target))
        .map(str::to_owned)
}

fn manifest_dependency<'a>(
    manifest: &'a toml_edit::DocumentMut,
    dependency: &cargo_metadata::Dependency,
) -> Option<&'a toml_edit::Item> {
    let section = dependency_section(dependency)?;
    let dependency_key = dependency.rename.as_deref().unwrap_or(&dependency.name);
    if let Some(target) = &dependency.target {
        let target_key = target_table_key(manifest, target)?;
        return manifest
            .get("target")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|targets| targets.get(&target_key))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|target| target.get(section))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|dependencies| dependencies.get(dependency_key));
    }

    manifest
        .get(section)
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|dependencies| dependencies.get(dependency_key))
}

fn manifest_dependency_mut<'a>(
    manifest: &'a mut toml_edit::DocumentMut,
    dependency: &cargo_metadata::Dependency,
) -> Option<&'a mut toml_edit::Item> {
    let section = dependency_section(dependency)?;
    let dependency_key = dependency.rename.as_deref().unwrap_or(&dependency.name);
    if let Some(target) = &dependency.target {
        let target_key = target_table_key(manifest, target)?;
        return manifest
            .get_mut("target")
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|targets| targets.get_mut(&target_key))
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|target| target.get_mut(section))
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|dependencies| dependencies.get_mut(dependency_key));
    }

    manifest
        .get_mut(section)
        .and_then(toml_edit::Item::as_table_like_mut)
        .and_then(|dependencies| dependencies.get_mut(dependency_key))
}

struct DependencyDeclaration {
    has_version: bool,
    inherits_workspace: bool,
}

fn dependency_declaration(
    manifest: &toml_edit::DocumentMut,
    workspace_manifest: &toml_edit::DocumentMut,
    package_name: &str,
    dependency: &cargo_metadata::Dependency,
) -> eyre::Result<DependencyDeclaration> {
    let dependency_key = dependency.rename.as_deref().unwrap_or(&dependency.name);
    let item = manifest_dependency(manifest, dependency).ok_or_else(|| {
        eyre::eyre!("{package_name}: dependency {dependency_key} is missing from its manifest")
    })?;

    let inherits_workspace = item
        .as_table_like()
        .and_then(|dependency| dependency.get("workspace"))
        .and_then(toml_edit::Item::as_bool)
        == Some(true);
    if !inherits_workspace {
        let has_version = item.is_str()
            || item
                .as_table_like()
                .is_some_and(|dependency| dependency.contains_key("version"));
        return Ok(DependencyDeclaration {
            has_version,
            inherits_workspace,
        });
    }

    let workspace_dependency = workspace_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|dependencies| dependencies.get(dependency_key))
        .ok_or_else(|| {
            eyre::eyre!(
                "{package_name}: workspace dependency {dependency_key} is missing from the workspace manifest"
            )
        })?;

    let has_version = workspace_dependency.is_str()
        || workspace_dependency
            .as_table_like()
            .is_some_and(|dependency| dependency.contains_key("version"));
    Ok(DependencyDeclaration {
        has_version,
        inherits_workspace,
    })
}

async fn prepare_package(
    package: &Arc<Package>,
    workspace_manifest: &toml_edit::DocumentMut,
    packages: &HashMap<PathBuf, Arc<Package>>,
    options: &Options,
) -> eyre::Result<()> {
    use toml_edit::DocumentMut;
    let manifest = tokio::fs::read_to_string(&package.inner.manifest_path).await?;
    let mut manifest = manifest.parse::<DocumentMut>()?;
    let mut need_update = false;

    for dependency in &package.inner.dependencies {
        let mut dependency_version = dependency.req.clone();
        if let Some(path) = dependency.path.as_ref().map(PathBuf::from) {
            // Resolve every local dependency, even if it already has a version requirement.
            let resolved = packages.get(&path).ok_or(eyre::eyre!(
                "{}: could not resolve local dependency {}",
                &package.inner.name,
                path.display()
            ))?;
            let declaration = dependency_declaration(
                &manifest,
                workspace_manifest,
                &package.inner.name,
                dependency,
            )?;

            // Cargo omits versionless development dependencies from published manifests,
            // which lets published packages keep private workspace-only test support.
            if dependency.kind == DependencyKind::Development
                && !declaration.has_version
                && !resolved.publishable
            {
                continue;
            }

            // A published package cannot depend on a local package excluded from this run.
            if !resolved.should_publish {
                eyre::bail!(
                    "{}: cannot publish because dependency {} will not be published",
                    &package.inner.name,
                    &dependency.name,
                );
            }

            if options.resolve_versions {
                // Use the version declared by the package at the dependency path.
                dependency_version = format!("={}", resolved.inner.version).parse()?;

                let changed = dependency_version != dependency.req;
                if changed && !declaration.inherits_workspace {
                    // Keep the manifest aligned with the graph used for publishing.
                    need_update |= update_dependency_version(
                        &mut manifest,
                        &package.inner.name,
                        dependency,
                        &dependency_version,
                    )?;
                }
            }

            package
                .deps
                .write()
                .insert(resolved.inner.name.to_string(), resolved.clone());

            resolved
                .dependants
                .write()
                .insert(package.inner.name.to_string(), package.clone());
        }

        let is_dev_dependency = dependency.kind == DependencyKind::Development;
        let is_non_local_dependency = !is_dev_dependency || dependency.path.is_none();
        let is_missing_exact_version = dependency_version == semver::VersionReq::STAR;

        if is_missing_exact_version && is_non_local_dependency {
            return Err(eyre::eyre!(
                "{}: dependency {} has no specific version ({})",
                &package.inner.name,
                &dependency.name,
                dependency_version
            ).suggestion("to automatically resolve versions of local workspace members, use '--resolve-versions'"));
        }
    }

    // Dry-runs validate the edits in memory without modifying the checkout.
    if !options.dry_run && need_update {
        use tokio::io::AsyncWriteExt;
        action::debug!("{}", &manifest.to_string());
        action::info!(
            "[{}@{}] updating {}",
            package.inner.name,
            package.inner.version,
            package.inner.manifest_path
        );
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&package.inner.manifest_path)
            .await?;
        file.write_all(manifest.to_string().as_bytes()).await?;
        file.flush().await?;
    }

    Ok(())
}

async fn build_dag(
    metadata: &cargo_metadata::Metadata,
    packages: &HashMap<PathBuf, Arc<Package>>,
    options: &Options,
) -> eyre::Result<()> {
    let workspace_manifest = tokio::fs::read_to_string(metadata.workspace_root.join("Cargo.toml"))
        .await?
        .parse::<toml_edit::DocumentMut>()?;
    let packages_iter = packages.values().filter(|package| package.should_publish);
    let results: Vec<_> = stream::iter(packages_iter)
        .map(|package| prepare_package(package, &workspace_manifest, packages, options))
        .buffer_unordered(8)
        .collect()
        .await;

    // Report any package error only after the bounded concurrent validation finishes.
    results.into_iter().collect::<eyre::Result<Vec<_>>>()?;
    Ok(())
}

/// Updates local path dependency versions in `[workspace.dependencies]`.
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
) -> eyre::Result<bool> {
    use toml_edit::{DocumentMut, Item, Value, value};

    // Fast path: nothing to do when resolve_versions is disabled.
    if !options.resolve_versions {
        return Ok(false);
    }

    let workspace_manifest_path = metadata.workspace_root.join("Cargo.toml");
    let manifest = tokio::fs::read_to_string(&workspace_manifest_path).await?;
    let mut manifest = manifest.parse::<DocumentMut>()?;
    let mut need_update = false;

    // Index package names once because every workspace dependency may need a lookup.
    let mut name_to_pkg: HashMap<String, &Arc<Package>> = HashMap::new();
    for pkg in packages.values() {
        name_to_pkg.insert(pkg.inner.name.to_string(), pkg);
    }

    let required_versions = packages
        .values()
        .filter(|package| package.should_publish)
        .flat_map(|package| &package.inner.dependencies)
        .filter(|dependency| {
            let Some(path) = dependency.path.as_ref().map(PathBuf::from) else {
                return false;
            };
            let Some(target) = packages.get(&path) else {
                return false;
            };

            dependency.kind != DependencyKind::Development
                || dependency.req != semver::VersionReq::STAR
                || target.publishable
        })
        .map(|dependency| dependency.name.as_str())
        .collect::<HashSet<_>>();

    if let Some(workspace) = manifest.get_mut("workspace")
        && let Some(deps_item) = workspace.get_mut("dependencies")
        && let Some(table) = deps_item.as_table_mut()
    {
        for (name, item) in table.iter_mut() {
            // Only consider workspace dependencies that correspond
            // to local workspace members.
            let dep_name = name.get().to_string();
            let package_name = item
                .as_inline_table()
                .and_then(|dependency| dependency.get("package"))
                .and_then(Value::as_str)
                .or_else(|| {
                    item.as_table()
                        .and_then(|dependency| dependency.get("package"))
                        .and_then(Item::as_str)
                })
                .unwrap_or(&dep_name)
                .to_string();
            let Some(pkg) = name_to_pkg.get(&package_name) else {
                continue;
            };
            if !required_versions.contains(package_name.as_str()) {
                continue;
            }

            // Skip simple string dependencies like `foo = "1"`.
            if item.is_str() {
                continue;
            }

            // Handle both inline tables and normal tables.
            if let Some(inline) = item.as_inline_table_mut() {
                let has_path = inline.get("path").is_some();
                let ver_req = format!("={}", pkg.inner.version);
                let has_resolved_version =
                    inline.get("version").and_then(Value::as_str) == Some(ver_req.as_str());

                if !has_path || has_resolved_version {
                    continue;
                }

                inline.insert("version", Value::from(ver_req));
                inline.fmt();
                need_update = true;
            } else if let Some(table_item) = item.as_table_mut() {
                let has_path = table_item.get("path").is_some();
                let ver_req = format!("={}", pkg.inner.version);
                let has_resolved_version =
                    table_item.get("version").and_then(Item::as_str) == Some(ver_req.as_str());

                if !has_path || has_resolved_version {
                    continue;
                }

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
        f.flush().await?;
    }

    Ok(need_update)
}

/// Publishes selected workspace packages to crates.io in dependency order.
///
/// Local path dependencies form the publishing graph. Independent packages run concurrently up to
/// [`Options::concurrency_limit`], while each dependant waits for its dependencies to appear on the
/// registry.
///
/// Versionless development dependencies on private packages are excluded because Cargo omits them
/// from the published manifest.
///
/// # Errors
///
/// Returns an error when the options or Cargo metadata are invalid, dependency versions cannot be
/// resolved, a selected package depends on an excluded package, `cargo publish` fails permanently,
/// or a published package does not become available before the registry timeout.
pub async fn publish(mut options: Options) -> eyre::Result<()> {
    options.validate()?;
    action::info!("searching cargo packages at {}", options.path.display());

    let manifest_path = if options.path.is_file() {
        options.path.clone()
    } else {
        options.path.join("Cargo.toml")
    };
    let mut metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&manifest_path)
        .exec()?;

    let mut packages: HashMap<PathBuf, Arc<Package>> = find_packages(&metadata, &options).collect();
    // For workspaces using `[workspace.dependencies]`, ensure local path
    // dependencies have explicit versions before we start publishing.
    let workspace_changed = update_workspace_dependencies(&metadata, &packages, &options).await?;
    if workspace_changed && !options.dry_run {
        // Cargo metadata retains the old requirements, so reload it after manifest mutation.
        metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()?;
        packages = find_packages(&metadata, &options).collect();
    }
    build_dag(&metadata, &packages, &options).await?;

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
        // Fast path: nothing to publish.
        return Ok(());
    }
    let mut ready: VecDeque<Arc<Package>> =
        packages.values().filter(|p| p.ready()).cloned().collect();

    let mut tasks: FuturesUnordered<Pin<Box<TaskFut>>> = FuturesUnordered::new();

    let limit = options.concurrency_limit.unwrap_or(4);
    let limit = Arc::new(Semaphore::new(limit));

    loop {
        // Completion
        if tasks.is_empty() && ready.is_empty() {
            break;
        }

        // Ready packages
        loop {
            // Concurrency slot
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                break;
            };
            // Package selection
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

                            // Release the concurrency slot before reporting completion.
                            drop(permit);
                            res
                        })
                    });
                }
                // No ready packages
                None => break,
            }
        }

        // Completed package
        match tasks.next().await {
            Some(Err(err)) => {
                eyre::bail!("a task failed: {}", err)
            }
            Some(Ok(completed)) => {
                // Newly unblocked dependants
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
    use std::path::{Path, PathBuf};

    use similar_asserts::assert_eq as sim_assert_eq;

    fn options(path: PathBuf) -> super::Options {
        super::Options {
            path,
            registry_token: None,
            dry_run: false,
            publish_delay: None,
            no_verify: false,
            resolve_versions: false,
            include: None,
            exclude: None,
            max_retries: None,
            concurrency_limit: None,
            extra_args: Vec::new(),
        }
    }

    fn write_member(workspace: &Path, name: &str, manifest_suffix: &str) {
        let package_dir = workspace.join("crates").join(name);
        std::fs::create_dir_all(package_dir.join("src"))
            .expect("package source directory must be created");
        std::fs::write(
            package_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{name}"
version = "1.2.3"
edition = "2021"

{manifest_suffix}"#
            ),
        )
        .expect("package manifest must be written");
        // Add a minimal lib target so Cargo considers this crate valid.
        std::fs::write(package_dir.join("src/lib.rs"), "").expect("package source must be written");
    }

    fn package_map(
        metadata: &cargo_metadata::Metadata,
        options: &super::Options,
    ) -> std::collections::HashMap<PathBuf, std::sync::Arc<super::Package>> {
        super::find_packages(metadata, options).collect()
    }

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

    #[test]
    fn package_selection_applies_exclusions_after_inclusions() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/foo", "crates/bar"]
resolver = "2"
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "foo", "");
        write_member(temp.path(), "bar", "");

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path);
        options.include = Some(vec!["foo".to_string(), "bar".to_string()]);
        options.exclude = Some(vec!["bar".to_string()]);

        let selected = package_map(&metadata, &options)
            .values()
            .map(|package| (package.inner.name.to_string(), package.should_publish))
            .collect::<std::collections::HashMap<_, _>>();

        sim_assert_eq!(selected.get("foo"), Some(&true));
        sim_assert_eq!(selected.get("bar"), Some(&false));
    }

    /// Versions normal and build dependencies, including renamed manifest keys.
    #[tokio::test]
    async fn build_dag_resolves_normal_and_build_dependencies() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/foo", "crates/build-support", "crates/consumer"]
resolver = "2"
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "foo", "");
        write_member(temp.path(), "build-support", "");
        write_member(
            temp.path(),
            "consumer",
            r#"[dependencies]
renamed = { package = "foo", path = "../foo" }

[build-dependencies]
build-support = { path = "../build-support" }
"#,
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path);
        options.resolve_versions = true;
        let packages = package_map(&metadata, &options);

        super::build_dag(&metadata, &packages, &options)
            .await
            .expect("dependency graph must resolve");

        let manifest = std::fs::read_to_string(
            temp.path()
                .join("crates")
                .join("consumer")
                .join("Cargo.toml"),
        )
        .expect("consumer manifest must be readable")
        .parse::<toml_edit::DocumentMut>()
        .expect("consumer manifest must remain valid TOML");
        let renamed = manifest
            .get("dependencies")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|dependencies| dependencies.get("renamed"))
            .and_then(toml_edit::Item::as_table_like)
            .expect("renamed dependency must remain a detailed entry");
        sim_assert_eq!(
            renamed.get("version").and_then(toml_edit::Item::as_str),
            Some("=1.2.3")
        );
        sim_assert_eq!(
            renamed.get("package").and_then(toml_edit::Item::as_str),
            Some("foo")
        );
        let build_support = manifest
            .get("build-dependencies")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|dependencies| dependencies.get("build-support"))
            .and_then(toml_edit::Item::as_table_like)
            .expect("build dependency must remain a detailed entry");
        sim_assert_eq!(
            build_support
                .get("version")
                .and_then(toml_edit::Item::as_str),
            Some("=1.2.3")
        );
    }

    /// Keeps path-only development dependencies on private packages out of the publish graph.
    #[tokio::test]
    async fn path_only_private_dev_dependency_is_omitted_from_publish_graph() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/test-support", "crates/consumer"]
resolver = "2"

[workspace.dependencies]
test-support = { path = "crates/test-support" }
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "test-support", "publish = false\n");
        write_member(
            temp.path(),
            "consumer",
            r"[dev-dependencies]
test-support.workspace = true
",
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path.clone());
        options.resolve_versions = true;
        let packages = package_map(&metadata, &options);

        let changed = super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .expect("private development dependencies must not require versions");
        super::build_dag(&metadata, &packages, &options)
            .await
            .expect("private development dependencies must not block publication");

        sim_assert_eq!(changed, false);
        let consumer = packages
            .values()
            .find(|package| package.inner.name == "consumer")
            .expect("consumer package must be present");
        sim_assert_eq!(consumer.deps.read().len(), 0);
        let manifest = std::fs::read_to_string(workspace_manifest_path)
            .expect("workspace manifest must be readable")
            .parse::<toml_edit::DocumentMut>()
            .expect("workspace manifest must remain valid TOML");
        let test_support = manifest
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(|dependencies| dependencies.get("test-support"))
            .expect("test support dependency must remain present");
        sim_assert_eq!(
            test_support
                .get("version")
                .and_then(toml_edit::Item::as_str),
            None
        );
    }

    /// Keeps directly declared path-only dev-dependencies on private packages untouched.
    #[tokio::test]
    async fn direct_path_only_private_dev_dependency_is_omitted() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/test-util", "crates/consumer"]
resolver = "2"
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "test-util", "publish = false\n");
        write_member(
            temp.path(),
            "consumer",
            r#"[dev-dependencies]
test-util = { path = "../test-util" }
"#,
        );
        let consumer_manifest_path = temp
            .path()
            .join("crates")
            .join("consumer")
            .join("Cargo.toml");
        let original_manifest = std::fs::read_to_string(&consumer_manifest_path)
            .expect("consumer manifest must be readable");

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path);
        options.resolve_versions = true;
        let packages = package_map(&metadata, &options);

        super::build_dag(&metadata, &packages, &options)
            .await
            .expect("private development dependencies must not block publication");

        let consumer = packages
            .values()
            .find(|package| package.inner.name == "consumer")
            .expect("consumer package must be present");
        sim_assert_eq!(consumer.deps.read().len(), 0);
        sim_assert_eq!(
            std::fs::read_to_string(&consumer_manifest_path)
                .expect("consumer manifest must be readable"),
            original_manifest
        );
    }

    /// Versions target-specific publishable dev-dependencies and preserves their ordering.
    #[tokio::test]
    async fn target_publishable_dev_dependency_is_versioned_and_ordered() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/test-support", "crates/consumer"]
resolver = "2"
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "test-support", "");
        // The spacing inside `cfg(...)` deliberately differs from the canonical platform
        // rendering to cover target keys that only match after parsing.
        write_member(
            temp.path(),
            "consumer",
            r#"[target.'cfg(target_os="linux")'.dev-dependencies]
test-support = { path = "../test-support" }
"#,
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path);
        options.resolve_versions = true;
        let packages = package_map(&metadata, &options);

        super::build_dag(&metadata, &packages, &options)
            .await
            .expect("publishable development dependency must resolve");

        let consumer = packages
            .values()
            .find(|package| package.inner.name == "consumer")
            .expect("consumer package must be present");
        sim_assert_eq!(consumer.deps.read().len(), 1);

        let manifest = std::fs::read_to_string(
            temp.path()
                .join("crates")
                .join("consumer")
                .join("Cargo.toml"),
        )
        .expect("consumer manifest must be readable")
        .parse::<toml_edit::DocumentMut>()
        .expect("consumer manifest must remain valid TOML");
        let test_support = manifest
            .get("target")
            .and_then(|targets| targets.get(r#"cfg(target_os="linux")"#))
            .and_then(|target| target.get("dev-dependencies"))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|dependencies| dependencies.get("test-support"))
            .and_then(toml_edit::Item::as_table_like)
            .expect("development dependency must remain a detailed entry");
        sim_assert_eq!(
            test_support
                .get("version")
                .and_then(toml_edit::Item::as_str),
            Some("=1.2.3")
        );
    }

    /// Rejects explicitly versioned dev-dependencies on private packages.
    #[tokio::test]
    async fn versioned_private_dev_dependency_is_rejected() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/test-support", "crates/consumer"]
resolver = "2"
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "test-support", "publish = false\n");
        write_member(
            temp.path(),
            "consumer",
            r#"[dev-dependencies]
test-support = { path = "../test-support", version = "*" }
"#,
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path);
        options.resolve_versions = true;
        let packages = package_map(&metadata, &options);

        let error = super::build_dag(&metadata, &packages, &options)
            .await
            .expect_err("versioned private development dependency must prevent publication");

        sim_assert_eq!(
            error.to_string(),
            "consumer: cannot publish because dependency test-support will not be published"
        );
    }

    /// Versions workspace-inherited development dependencies on publishable packages.
    #[tokio::test]
    async fn workspace_publishable_dev_dependency_receives_version() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/test-support", "crates/consumer"]
resolver = "2"

[workspace.dependencies]
test-support = { path = "crates/test-support" }
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "test-support", "");
        write_member(
            temp.path(),
            "consumer",
            r"[dev-dependencies]
test-support.workspace = true
",
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let mut options = options(workspace_manifest_path.clone());
        options.resolve_versions = true;
        let packages = package_map(&metadata, &options);

        let changed = super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .expect("publishable development dependency must resolve");

        sim_assert_eq!(changed, true);
        let manifest = std::fs::read_to_string(workspace_manifest_path)
            .expect("workspace manifest must be readable")
            .parse::<toml_edit::DocumentMut>()
            .expect("workspace manifest must remain valid TOML");
        let test_support = manifest
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(|dependencies| dependencies.get("test-support"))
            .expect("development dependency must remain present");
        sim_assert_eq!(
            test_support
                .get("version")
                .and_then(toml_edit::Item::as_str),
            Some("=1.2.3")
        );
    }

    /// Rejects private local packages used as production dependencies.
    #[tokio::test]
    async fn private_normal_dependency_is_rejected() {
        let temp = tempfile::tempdir().expect("temporary workspace must be created");
        let workspace_manifest_path = temp.path().join("Cargo.toml");
        std::fs::write(
            &workspace_manifest_path,
            r#"[workspace]
members = ["crates/private-lib", "crates/consumer"]
resolver = "2"
"#,
        )
        .expect("workspace manifest must be written");
        write_member(temp.path(), "private-lib", "publish = false\n");
        write_member(
            temp.path(),
            "consumer",
            r#"[dependencies]
private-lib = { path = "../private-lib" }
"#,
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .expect("workspace metadata must load");
        let options = options(workspace_manifest_path);
        let packages = package_map(&metadata, &options);

        let error = super::build_dag(&metadata, &packages, &options)
            .await
            .expect_err("private production dependencies must prevent publication");

        sim_assert_eq!(
            error.to_string(),
            "consumer: cannot publish because dependency private-lib will not be published"
        );
    }

    /// Verifies workspace-level path dependencies are resolved without changing unrelated entries.
    #[tokio::test]
    async fn update_workspace_dependencies_adds_versions_for_local_path_deps() {
        // Synthetic workspace manifest with a path-only workspace dependency
        // and a normal crates.io dependency that should be left untouched.
        let workspace_manifest = r#"
[workspace]
members = ["crates/foo", "crates/bar", "crates/consumer"]

[workspace.package]
version = "1.2.3"

[workspace.dependencies]
foo-alias = { package = "foo", path = "crates/foo", version = "*" }
serde = "1"

[workspace.dependencies.bar]
path = "crates/bar"
"#;

        // Create a temporary workspace directory with Cargo.toml.
        let tmp = tempfile::tempdir().unwrap();
        let workspace_manifest_path = tmp.path().join("Cargo.toml");
        std::fs::write(&workspace_manifest_path, workspace_manifest).unwrap();

        // Minimal dummy member manifests so that cargo_metadata can see the packages.
        // Member `foo` for the inline table workspace dependency.
        write_member(tmp.path(), "foo", "");

        // Member `bar` for the table-style workspace dependency.
        write_member(tmp.path(), "bar", "");
        write_member(
            tmp.path(),
            "consumer",
            r"[dependencies]
foo-alias.workspace = true
bar.workspace = true
",
        );

        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&workspace_manifest_path)
            .exec()
            .unwrap();

        // Build a packages map compatible with update_workspace_dependencies.
        let packages = package_map(&metadata, &options(workspace_manifest_path.clone()));

        let mut options = options(workspace_manifest_path.clone());
        options.resolve_versions = true;

        // Exercise the helper in dry-run mode before allowing it to persist changes.
        let original_manifest = std::fs::read_to_string(&workspace_manifest_path).unwrap();
        options.dry_run = true;
        let changed = super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .unwrap();
        sim_assert_eq!(changed, true);
        sim_assert_eq!(
            std::fs::read_to_string(&workspace_manifest_path).unwrap(),
            original_manifest
        );

        options.dry_run = false;
        let changed = super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .unwrap();
        sim_assert_eq!(changed, true);

        let updated_manifest = std::fs::read_to_string(&workspace_manifest_path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let dependencies = updated_manifest
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies"))
            .expect("workspace dependencies must remain present");
        let foo_alias = dependencies
            .get("foo-alias")
            .expect("foo alias must remain a detailed entry");
        sim_assert_eq!(
            foo_alias.get("version").and_then(toml_edit::Item::as_str),
            Some("=1.2.3")
        );
        sim_assert_eq!(
            foo_alias.get("package").and_then(toml_edit::Item::as_str),
            Some("foo")
        );
        sim_assert_eq!(
            dependencies
                .get("bar")
                .and_then(|bar| bar.get("version"))
                .and_then(toml_edit::Item::as_str),
            Some("=1.2.3")
        );
        sim_assert_eq!(
            dependencies.get("serde").and_then(toml_edit::Item::as_str),
            Some("1")
        );

        let changed = super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .unwrap();
        sim_assert_eq!(changed, false);

        // Exercise the helper again with resolve_versions disabled; this should
        // also be a no-op without errors.
        options.resolve_versions = false;
        let changed = super::update_workspace_dependencies(&metadata, &packages, &options)
            .await
            .unwrap();
        sim_assert_eq!(changed, false);
    }
}
