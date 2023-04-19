// #![allow(warnings)]
#![allow(clippy::missing_panics_doc)]

use action_core as action;
use cargo_metadata::DependencyKind;
use color_eyre::eyre;
use futures::stream::{self, FuturesUnordered, StreamExt};
use futures::Future;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::Semaphore;
use tokio::time::{interval, sleep, Duration, Instant};

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
}

/// A cargo package.
struct Package {
    package: cargo_metadata::Package,
    path: PathBuf,
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
            .field("name", &self.package.name)
            .field("version", &self.package.version.to_string())
            .field(
                "deps",
                &self.deps.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .field(
                "dependants",
                &self.dependants.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

// #[derive(thiserror::Error, Debug)]
// enum WaitForPackageError {
//     #[error("timeout waiting for create to be published")]
//     Timeout,
// }

impl Package {
    /// Returns `true` if the package has been successfully published.
    pub fn published(&self) -> bool {
        *self.published.lock().unwrap()
    }

    /// Checks if the package is ready for publishing.
    ///
    /// A package can be published if all its dependencies have been published.
    pub fn ready(&self) -> bool {
        self.deps.read().unwrap().values().all(|d| d.published())
    }

    /// Wait until the published package is available on the registry.
    pub async fn is_available(&self) -> eyre::Result<bool> {
        use crates_io_api::{AsyncClient, Error as RegistryError};
        use semver::Version;

        let api = AsyncClient::new(
            "publish_crates (https://github.com/romnn/publish-crates)",
            std::time::Duration::from_millis(1000),
        )?;

        let info = match api.get_crate(&self.package.name).await {
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
        let Some((_, version))= versions.find(|(ver, _)| ver == &self.package.version) else {
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
                "checking if {} {} is available",
                self.package.name,
                self.package.version.to_string()
            );
            if self.is_available().await? {
                return Ok(());
            }
            // check for timeout
            if Instant::now().duration_since(start) > timeout {
                eyre::bail!(
                    "exceeded timeout of {:?} waiting for crate {} {} to be published",
                    timeout,
                    self.package.name,
                    self.package.version.to_string()
                );
            }
        }
    }

    /// Publishes this package
    pub async fn publish(self: Arc<Self>, options: Arc<Options>) -> eyre::Result<Arc<Self>> {
        use async_process::Command;

        action::info!("publishing {}", self.package.name,);

        let mut cmd = Command::new("cargo");
        cmd.arg("publish");
        // cmd.args(options.args);

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
            // if !self.deps.read().unwrap().is_empty() {
            //     cmd.arg("--offline");
            // }
        }
        if options.resolve_versions {
            // when resolving versions, we may write to Cargo.toml
            cmd.arg("--allow-dirty");
        }
        let output = cmd.output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        action::debug!("{}", &stdout);
        action::debug!("{}", &stderr);

        if !output.status.success() {
            eyre::bail!("command {:?} failed: {}", cmd, stderr);
        }

        if options.dry_run {
            action::info!(
                "dry-run: skipping waiting for {} {} to be published",
                &self.package.name,
                self.package.version
            );
            *self.published.lock().unwrap() = true;
            return Ok(self);
        }

        // wait for package to be available on the registry
        self.wait_package_available(None).await?;

        sleep(
            options
                .publish_delay
                .unwrap_or_else(|| Duration::from_secs(30)),
        )
        .await;

        let mut cmd = Command::new("cargo");
        cmd.arg("update");
        cmd.current_dir(&self.path);
        let output = cmd.output().await?;
        if !output.status.success() {
            eyre::bail!("command {:?} failed", cmd);
        }

        *self.published.lock().unwrap() = true;
        action::info!("published {}", self.package.name);

        Ok(self)
    }
}

type TaskFut = dyn Future<Output = eyre::Result<Arc<Package>>>;

fn find_packages(
    metadata: &cargo_metadata::Metadata,
    options: Arc<Options>,
) -> impl Iterator<Item = (PathBuf, Arc<Package>)> + '_ {
    let packages = metadata.workspace_packages();
    packages.into_iter().filter_map(move |package| {
        if let Some(publish) = &package.publish {
            // publish = ["some-registry-name"]
            // The value may also be an array of strings
            // which are registry names that are allowed to be published to.
            if publish.is_empty() {
                // skip package
                action::info!("skipping: {} (publish=false)", package.name);
                return None;
            }
        }
        if let Some(include) = &options.include {
            if !include.is_empty() && !include.contains(&package.name) {
                // skip package
                action::info!("skipping: {} (not included)", package.name);
                return None;
            }
        }
        if let Some(exclude) = &options.exclude {
            if exclude.contains(&package.name) {
                // skip package
                action::info!("skipping: {} (excluded)", package.name);
                return None;
            }
        }
        let path: PathBuf = package.manifest_path.parent()?.into();
        Some((
            path.clone(),
            Arc::new(Package {
                package: package.clone(),
                path,
                published: Mutex::new(false),
                deps: RwLock::new(HashMap::new()),
                dependants: RwLock::new(HashMap::new()),
            }),
        ))
    })
}

async fn build_dag(
    packages: Arc<HashMap<PathBuf, Arc<Package>>>,
    options: Arc<Options>,
) -> eyre::Result<()> {
    let results: Vec<_> = stream::iter(packages.values())
        .map(|p| {
            let packages = packages.clone();
            let options = options.clone();
            async move {
                use toml_edit::{value, Document};
                let manifest_path = &p.package.manifest_path;
                let manifest = tokio::fs::read_to_string(manifest_path).await?;
                let mut manifest = manifest.parse::<Document>()?;
                let mut need_update = false;

                for dep in &p.package.dependencies {
                    let mut dep_version = dep.req.clone();
                    if let Some(path) = dep.path.as_ref().map(PathBuf::from) {
                        // also if the version is set, we want to resolve automatically?
                        // OR we allow changing and always set allow-dirty
                        // dep_version == semver::VersionReq::STAR &&
                        let resolved = packages.get(&path).ok_or(eyre::eyre!(
                            "{}: could not resolve local dependency {}",
                            &p.package.name,
                            path.display()
                        ))?;
                        if options.resolve_versions {
                            // use version from the manifest the path points to
                            dep_version = semver::VersionReq {
                                comparators: vec![semver::Comparator {
                                    op: semver::Op::Exact,
                                    major: resolved.package.version.major,
                                    minor: Some(resolved.package.version.minor),
                                    patch: Some(resolved.package.version.patch),
                                    pre: semver::Prerelease::EMPTY,
                                }],
                            };

                            let changed = dep_version != dep.req;
                            if changed {
                                // update cargo manifest
                                if let Some(kind) = match dep.kind {
                                    DependencyKind::Normal => Some("dependencies"),
                                    DependencyKind::Development => Some("dev-dependencies"),
                                    DependencyKind::Build => Some("build-dependencies"),
                                    _ => None,
                                } {
                                    // TODO: !!!! do not remove the path thing here!
                                    manifest[kind][&dep.name]["version"] =
                                        value(dep_version.to_string());
                                    manifest[kind][&dep.name]
                                        .as_inline_table_mut()
                                        .map(toml_edit::InlineTable::fmt);
                                    need_update = true;
                                }
                            }
                        }

                        p.deps
                            .write()
                            .unwrap()
                            .insert(resolved.package.name.clone(), resolved.clone());

                        resolved
                            .dependants
                            .write()
                            .unwrap()
                            .insert(p.package.name.clone(), p.clone());
                    }

                    if dep_version == semver::VersionReq::STAR
                        && (dep.kind != DependencyKind::Development || dep.path.is_none())
                    {
                        eyre::bail!(
                            "{}: dependency {} is missing version field",
                            &p.package.name,
                            &dep.name
                        );
                    }
                }

                // write updated cargo manifest
                if need_update {
                    use tokio::io::AsyncWriteExt;
                    action::debug!("{}", &manifest.to_string());
                    action::warning!("{}: updating {}", &p.package.name, &p.package.manifest_path);
                    let mut f = tokio::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&p.package.manifest_path)
                        .await?;
                    f.write_all(manifest.to_string().as_bytes()).await?;
                }

                Ok(())
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;

    // fail on error
    results.into_iter().collect::<eyre::Result<Vec<_>>>()?;
    Ok(())
}

/// Publishes packages of a project on crates.io.
///
/// # Errors
/// If any package cannot be published.
pub async fn publish(options: Arc<Options>) -> eyre::Result<()> {
    action::info!("searching cargo packages at {}", options.path.display());

    let manifest_path = if options.path.is_file() {
        options.path.clone()
    } else {
        options.path.join("Cargo.toml")
    };
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&manifest_path)
        .exec()?;

    let packages: Arc<HashMap<PathBuf, Arc<Package>>> =
        Arc::new(find_packages(&metadata, options.clone()).collect::<HashMap<_, _>>());

    build_dag(packages.clone(), options.clone()).await?;

    action::info!(
        "found packages: {:?}",
        packages
            .values()
            .map(|p| p.package.name.clone())
            .collect::<Vec<_>>()
    );

    if packages.is_empty() {
        // fast path: nothing to do here
        return Ok(());
    }
    let mut ready: VecDeque<Arc<Package>> =
        packages.values().filter(|p| p.ready()).cloned().collect();

    let mut tasks: FuturesUnordered<Pin<Box<TaskFut>>> = FuturesUnordered::new();
    let limit = Arc::new(Semaphore::new(5));

    loop {
        // check if we are done
        if tasks.is_empty() && ready.is_empty() {
            break;
        }

        // start running ready tasks
        loop {
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                break;
            };
            // tokio::sync::TryAcquireError::NoPermits

            // check if we can publish
            match ready.pop_front() {
                Some(p) => {
                    let options_clone = options.clone();
                    tasks.push(Box::pin(async move {
                        let res = p.publish(options_clone).await;
                        drop(permit);
                        res
                    }));
                }
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
                        .unwrap()
                        .values()
                        .filter(|d| d.ready() && !d.published())
                        .cloned(),
                );
            }
            None => {}
        }
    }

    if !packages.values().all(|p| p.published()) {
        eyre::bail!("not all published");
    }

    Ok(())
}
