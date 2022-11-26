// #![allow(warnings)]

use actions_toolkit::core as actions;
use anyhow::Result;
use cargo_metadata::DependencyKind;
use futures::stream::{self, FuturesUnordered, StreamExt};
use futures::Future;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use tokio::time::{interval, sleep, Duration, Instant};

/// Options for publishing packages.
#[derive(Debug)]
pub struct Options {
    /// Path to the package or workspace to be considered
    pub path: PathBuf,
    /// GitHub token
    pub token: String,
    /// Cargo registry token
    pub registry_token: Option<String>,
    /// Perform dry-run
    /// This will perform all checks without publishing the package
    pub dry_run: bool,
    /// todo
    pub check_repo: bool,
    /// Delay before attempting to publish dependent crate
    pub publish_delay: Option<u16>,
    /// todo
    pub no_verify: bool,
    /// Resolve missing versions for local packages.
    ///
    /// Versions of local packages that use `{ path = "../some/path" }`
    /// will be resolved to the version of the package the `path` is pointing to.
    /// Note that even if `version` is present, the resolved value will be used.
    ///
    /// **Note**: This will update your `Cargo.toml` manifest with the resolved version.
    pub resolve_versions: bool,
    /// todo
    pub ignore_unpublished: bool,
    /// Packages to explicitely include
    ///
    /// If using explicit include, specify all package names you wish to publish
    pub include: Option<Vec<String>>,
    /// Packages to explicitely exclude
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
        write!(f, "{}", self)
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
    pub async fn is_available(&self) -> Result<bool> {
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
        let version = match versions.find(|(ver, _)| ver == &self.package.version) {
            Some((_, version)) => version,
            None => return Ok(false),
        };

        let client = reqwest::Client::new();
        let dl_response = client
            .head(format!("https://crates.io{}", version.dl_path))
            .send()
            .await?;
        Ok(dl_response.status() == reqwest::StatusCode::OK)
    }

    /// Wait until the published package is available on the registry.
    pub async fn wait_package_available(&self, timeout: impl Into<Option<Duration>>) -> Result<()> {
        let timeout = timeout
            .into()
            .unwrap_or_else(|| Duration::from_secs(2 * 60));
        let start = Instant::now();
        let mut ticker = interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            println!("tick");
            if self.is_available().await? {
                return Ok(());
            }
            // check for timeout
            if Instant::now().duration_since(start) > timeout {
                anyhow::bail!(
                    "exceeded timeout of {:?} waiting for crate {} {} to be published",
                    timeout,
                    self.package.name,
                    self.package.version.to_string()
                );
            }
        }
        unreachable!();
    }

    /// Publishes this package
    pub async fn publish(self: Arc<Self>, options: Arc<Options>) -> Result<Arc<Self>> {
        use async_process::Command;

        println!("publishing {}", self.path.display());

        // wait for package to be available on the registry
        self.wait_package_available(None).await?;

        if let Some(delay) = options.publish_delay {
            sleep(Duration::from_secs(delay as u64)).await;
        }
        let mut cmd = Command::new("cargo");
        cmd.arg("update");
        cmd.current_dir(&self.path);
        let output = cmd.output().await?;
        if !output.status.success() {
            anyhow::bail!("command {:?} failed", cmd);
        }

        *self.published.lock().unwrap() = true;
        println!("published {}", self.package.name);
        return Ok(self);

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
        }
        if options.resolve_versions {
            cmd.arg("--allow-dirty");
        }
        println!("publishing {}", self.package.name);
        let output = cmd.output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("stdout {}", stdout);
        println!("stdout {}", stderr);

        if !output.status.success() {
            anyhow::bail!("command {:?} failed: {}", cmd, stderr);
        }

        if options.dry_run {
            println!(
                "dry-run: skipping waiting for {} {} to be published",
                &self.package.name, self.package.version
            );
            *self.published.lock().unwrap() = true;
            return Ok(self);
        }
        // wait for the package to become available here
        *self.published.lock().unwrap() = true;
        Ok(self)
    }
}

pub async fn test(options: Arc<Options>) -> Result<()> {
    println!("searching cargo packages at {}", options.path.display());
    // actions::log_message(actions::LogLevel::Debug, "test");

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&options.path)
        .exec()?;

    let packages = metadata.workspace_packages();
    let packages: Arc<HashMap<PathBuf, Arc<Package>>> = Arc::new(
        stream::iter(packages)
            .filter_map(|package| {
                let options = options.clone();
                async move {
                    if let Some(publish) = &package.publish {
                        // publish = ["some-registry-name"]
                        // The value may also be an array of strings
                        // which are registry names that are allowed to be published to.
                        if publish.is_empty() {
                            // skip package
                            println!("exluding: {}", package.name);
                            return None;
                        }
                    }
                    if let Some(include) = &options.include {
                        if include.len() > 0 {
                            if !include.contains(&package.name) {
                                // skip package
                                println!("exluding: {}", package.name);
                                return None;
                            }
                        }
                    }
                    if let Some(exclude) = &options.exclude {
                        if exclude.contains(&package.name) {
                            // skip package
                            println!("exluding: {}", package.name);
                            return None;
                        }
                    }
                    let path: PathBuf = package.manifest_path.parent().unwrap().into();
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
                }
            })
            .collect()
            .await,
    );

    let results: Vec<_> = stream::iter(packages.values())
        .map(|p| {
            let packages = packages.clone();
            let options = options.clone();
            async move {
                use toml_edit::{value, Document};
                let manifest = tokio::fs::read_to_string(&p.package.manifest_path).await?;
                let mut manifest = manifest.parse::<Document>()?;
                let mut need_update = false;

                for dep in &p.package.dependencies {
                    let mut dep_version = dep.req.clone();
                    if let Some(path) = dep.path.as_ref().map(PathBuf::from) {
                        // also if the version is set, we want to resolve automatically?
                        // OR we allow chaning and always set allow-dirty
                        // dep_version == semver::VersionReq::STAR &&
                        let resolved = packages.get(&path).ok_or(anyhow::anyhow!(
                            "could not resolve local dependency {}",
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
                                    manifest[kind][&dep.name]["version"] =
                                        value(dep_version.to_string());
                                    manifest[kind][&dep.name]
                                        .as_inline_table_mut()
                                        .map(|t| t.fmt());
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

                    if dep_version == semver::VersionReq::STAR {
                        if dep.kind != DependencyKind::Development {
                            anyhow::bail!("dependency {} is missing version field", &dep.name);
                        } else if dep.path.is_none() {
                            anyhow::bail!("dependency {} is missing version field", &dep.name);
                        }
                    }
                }

                // write updated cargo manifest
                if need_update {
                    // println!("{}", &manifest.to_string());
                    println!("updating {}", &p.package.manifest_path);
                    if false {
                        use tokio::io::AsyncWriteExt;
                        let mut f = tokio::fs::OpenOptions::new()
                            .write(true)
                            .truncate(true)
                            .open(&p.package.manifest_path)
                            .await?;
                        f.write_all(manifest.to_string().as_bytes()).await?;
                    }
                }

                Ok(())
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;

    // fail on error
    results.into_iter().collect::<Result<Vec<_>>>()?;

    println!("{:#?}", &packages);

    println!(
        "found packages: {:?}",
        packages
            .values()
            .map(|p| p.package.name.clone())
            .collect::<Vec<_>>()
    );

    // // checking is not really necessary?
    // // because we run --dry-run with cargo
    // //  => does that check for versions already published
    // //  => build and dep releated issues will be detected due to it packaging...

    if packages.is_empty() {
        // fast path: nothing to do here
        return Ok(());
    }
    let mut ready: Vec<Arc<Package>> = packages.values().filter(|p| p.ready()).cloned().collect();

    type TaskFut = dyn Future<Output = Result<Arc<Package>>>;
    let mut tasks: FuturesUnordered<Pin<Box<TaskFut>>> = FuturesUnordered::new();

    loop {
        // check if we are done
        if tasks.is_empty() && ready.is_empty() {
            break;
        }

        // start running ready tasks
        for p in ready.drain(0..) {
            let options_clone = options.clone();
            tasks.push(Box::pin(async move { p.publish(options_clone).await }));
        }

        // wait for a task to complete
        match tasks.next().await {
            Some(Err(err)) => {
                anyhow::bail!("a task failed: {}", err)
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
        anyhow::bail!("not all published");
    }

    Ok(())
}
