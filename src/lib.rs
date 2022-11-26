#![allow(warnings)]

use actions_toolkit::core as actions;
use anyhow::Result;
use cargo_metadata::{Dependency, DependencyKind};
// use futures::lock::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
// use std::sync::RwLock;
use tokio::sync::{Mutex, RwLock};

#[derive(Debug)]
pub struct Options {
    pub path: PathBuf,
    pub token: String,
    pub registry_token: Option<String>,
    pub dry_run: bool,
    pub check_repo: bool,
    pub publish_delay: Option<u16>,
    pub no_verify: bool,
    pub resolve_versions: bool,
    pub ignore_unpublished: bool,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

use std::sync::Arc;

// must be able to share the package async
#[derive()]
// struct Package<'a> {
struct Package {
    // package: &'a cargo_metadata::Package,
    package: cargo_metadata::Package,
    path: PathBuf,
    published: Mutex<bool>,
    // deps: HashMap<PathBuf, Arc<RwLock<Package>>>,
    // deps: HashMap<PathBuf, Arc<Package>>,
    deps: RwLock<HashMap<String, Arc<Package>>>,
    // deps: Mutex<HashMap<String, &'a Package<'a>>>,
    // deps: MuteHashMap<String, Arc<Package>>,
    // dependants: HashMap<PathBuf, Arc<RwLock<Package>>>,
    dependants: RwLock<HashMap<String, Arc<Package>>>,
    // dependants: Mutex<HashMap<String, &'a Package<'a>>>,
    // dependants: HashMap<String, Arc<Package>>,
}

// impl<'a> std::fmt::Debug for Package<'a> {
impl std::fmt::Debug for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "{}", self)
    }
}

// impl<'a> std::fmt::Display for Package<'a> {
impl std::fmt::Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Package")
            .field("name", &self.package.name)
            .field("version", &self.package.version.to_string())
            // .field(
            //     "deps",
            //     // &self.deps().keys().collect::<Vec<_>>(),
            //     &self.deps.read().unwrap().keys().collect::<Vec<_>>(),
            // )
            // .field(
            //     "dependants",
            //     &self.dependants.read().unwrap().keys().collect::<Vec<_>>(),
            // )
            .finish()
    }
}

// impl<'a> Package<'a> {
impl Package {
    // pub fn deps(&self) -> Vec<HashMap< Arc<Package>> {
    //     self.deps.read().unwrap().values().cloned().collect()
    // }

    // pub fn dependants(&self) -> Vec<Arc<Package>> {
    //     self.dependants.read().unwrap().values().cloned().collect()
    // }

    pub async fn published(&self) -> bool {
        *self.published.lock().await
    }

    // pub async fn ready(self: &Arc<Self>) -> bool {
    pub async fn ready(&self) -> bool {
        // check if all dependants are published
        stream::iter(self.deps.read().await.values()) // unwrap().values())
            .all(|d| async move { *d.published.lock().await })
            .await
    }

    // pub async fn publish(&self, options: &Options) -> Result<&Package<'a>> {
    // pub async fn publish<'a, 'o>(
    pub async fn publish(
        // self: &'a Arc<Self>,
        self: Arc<Self>,
        options: Arc<Options>,
        // options: &'o Options,
    ) -> Result<Arc<Self>> {
        use std::process::Command;

        println!("publishing {}", self.path.display());
        *self.published.lock().await = true;
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
            // println!("dry-run: skipping executing {:?}", &cmd);
            // continue;
        }
        if options.resolve_versions {
            cmd.arg("--allow-dirty");
        }
        println!("publishing {}", self.package.name);
        let output = cmd.output()?;
        println!("stdout {}", String::from_utf8_lossy(&output.stdout));
        println!("stdout {}", String::from_utf8_lossy(&output.stderr));
        if options.dry_run {
            println!(
                "dry-run: skipping waiting for {} {} to be published",
                &self.package.name, self.package.version
            );
            *self.published.lock().await = true;
            return Ok(self);
        }
        // wait for the package to become available here
        *self.published.lock().await = true;
        Ok(self)
    }
}

use futures::stream::{self, FuturesUnordered, StreamExt};

pub async fn test(options: Arc<Options>) -> Result<()> {
    println!("searching cargo packages at {}", options.path.display());
    // actions::log_message(actions::LogLevel::Debug, "test");

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&options.path)
        .exec()?;
    // dbg!(&metadata);

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
                            // deps: HashMap::new(),
                            dependants: RwLock::new(HashMap::new()),
                            // dependants: HashMap::new(),
                        }),
                    ))
                }
            })
            .collect()
            .await,
    );

    // todo: should we just make the full package an rwlock?
    // should we just make all the packages arcs? and clone them once?

    // let mut dependencies
    // let mut dependants

    // for p in packages.values() {
    let results: Vec<_> = stream::iter(packages.values())
        .map(|p| {
            let packages = packages.clone();
            let options = options.clone();
            async move {
                use toml_edit::{value, Document};
                let mut manifest =
                    std::fs::read_to_string(&p.package.manifest_path)?.parse::<Document>()?;
                let mut need_update = false;
                let mut dependencies: Vec<PathBuf> = Vec::new();

                for dep in &p.package.dependencies {
                    dbg!(&dep.path);
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
                                }
                            }
                        }

                        dependencies.push(path); // resolved.package.name.clone());

                        p.deps
                            .write()
                            .await
                            // .unwrap()
                            .insert(resolved.package.name.clone(), resolved.clone());

                        resolved
                            .dependants
                            .write()
                            .await
                            // .unwrap()
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
                    println!("{}", &manifest.to_string());
                }

                // Ok((p.package.name.clone(), dependencies))
                // Ok((p.package.name.clone(), dependencies))
                // Ok((p.path.clone(), dependencies))
                Ok(())
                // p.package.manifest_path
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;

    // for result in results.into_iter() {
    //     match result {
    //         Ok((pkg_path, deps)) => {
    //             let pkg = packages.get_mut(&pkg_path).unwrap();
    //             for dep_path in deps {
    //                 let dep = packages.get(&dep_path).unwrap();
    //                 pkg.deps
    //                     // .write()
    //                     // .await
    //                     // .unwrap()
    //                     .insert(dep_path, dep.clone());

    //                 dep.dependants
    //                     // .write()
    //                     // .await
    //                     // .unwrap()
    //                     .insert(pkg_path, pkg.clone());
    //             }
    //         }
    //         Err(err) => return Err(err),
    //     }
    // }
    results.into_iter().collect::<Result<Vec<_>>>()?;

    // update dependency graphs
    println!("{:#?}", &packages);
    dbg!(&packages.len());

    // we access dependants (read) -> its dependencies (read)
    // if a dependency is running, it is write locked and we deadlock
    // -> package needs interior mutability

    // let package_names: Vec<PathBuf> = packages.keys().collect();
    println!(
        "found packages: {:?}",
        packages
            .values()
            .map(|p| p.package.name.clone())
            .collect::<Vec<_>>()
    );
    // println!("found packages: {:?}", packages.keys().collect::<Vec<_>>());

    // // checking is not really necessary?
    // // because we run --dry-run with cargo
    // //  => does that check for versions already published
    // //  => build and dep releated issues will be detected due to it packaging...

    if packages.is_empty() {
        // fast path: nothing to do here
        return Ok(());
    }
    // let mut ready = publish_packages
    //     .iter()
    //     .filter(|(_, p)| p.deps.lock().unwrap().is_empty())
    //     .collect::<HashMap<_, _>>();

    let mut ready: Vec<Arc<Package>> = Vec::new();
    for p in packages.values() {
        if p.ready().await {
            ready.push(p.clone());
        }
    }
    // let mut ready: Vec<Arc<Package>> = stream::iter(packages.values().cloned())
    //     // .filter(|p| p.deps.lock().unwrap().is_empty())
    //     // .filter(|p: Arc<Package>| async move { p.ready().await })
    //     .filter(|p| async move {
    //         // None
    //         // p.ready().await
    //         if stream::iter(p.deps.read().await.values())
    //             .all(|d| async move { *d.published.lock().await })
    //             .await
    //         {
    //             true
    //             // Some(p)
    //         } else {
    //             false
    //             // None
    //         }
    //     })
    //     // .buffer_unordered(8)
    //     .collect::<Vec<_>>()
    //     .await;

    if ready.is_empty() {
        anyhow::bail!("cycles? cannot start");
    }

    use futures::stream::{FuturesUnordered, StreamExt};
    use futures::Future;

    // // let mut tasks: FuturesUnordered<Pin<Box<dyn Future<Output = Result<&'_ Package<'a>>>>>> =
    // // type TaskFut<'a> = dyn Future<Output = Result<&'a Arc<Package>>>;
    type TaskFut = dyn Future<Output = Result<Arc<Package>>>;
    let mut tasks: FuturesUnordered<Pin<Box<TaskFut>>> = FuturesUnordered::new();

    loop {
        // push ready tasks
        for p in ready.drain(0..) {
            let options_clone = options.clone();
            tasks.push(
                // p.publish(&options))
                Box::pin(async move { p.clone().publish(options_clone).await }),
            );
        }
        assert!(ready.is_empty());

        // wait for task to complete
        // let test = tasks.next().await;
        match tasks.next().await {
            Some(Err(err)) => {
                anyhow::bail!("a task failed: {}", err)
            }
            Some(Ok(completed)) => {
                // update ready tasks
                for d in completed.dependants.read().await.values() {
                    if d.ready().await {
                        ready.push(d.clone());
                    }
                }
                // ready.extend(
                //     stream::iter(completed.dependants.read().await.values().cloned())
                //         .filter(|d| d.ready())
                //         .collect::<Vec<Arc<Package>>>()
                //         .await,
                // );
                // .for_each(|d| async move { ready.push(d) });
                // .await;
                // }
                // for d in completed.dependants.read().unwrap().values() {
                //     if d.ready().await {
                //         ready.push(d);
                //     }
                // }
            }
            None => {}
        }

        // check if we are done
        if tasks.is_empty() && ready.is_empty() {
            break;
        }
    }

    if !stream::iter(packages.values()).all(|p| p.published()).await {
        // async move *p.published.lock().unwrap()) {
        anyhow::bail!("not all published");
    }

    Ok(())
}
