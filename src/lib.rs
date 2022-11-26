#![allow(warnings)]

use actions_toolkit::core as actions;
use anyhow::Result;
use cargo_metadata::{Dependency, DependencyKind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

// #[derive(Debug, Hash, Eq, PartialEq)]
#[derive()]
struct Package<'a> {
    package: &'a cargo_metadata::Package,
    path: PathBuf,
    deps: Mutex<HashMap<String, &'a Package<'a>>>,
    dependants: Mutex<HashMap<String, &'a Package<'a>>>,
    // local_deps: Mutex<HashMap<String, LocalDependency>>,
}

impl<'a> std::fmt::Debug for Package<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "{}", self)
    }
}

impl<'a> std::fmt::Display for Package<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Package")
            .field("name", &self.package.name)
            .field("version", &self.package.version.to_string())
            .field(
                "deps",
                &self.deps.lock().unwrap().keys().collect::<Vec<_>>(),
            )
            .field(
                "dependants",
                &self.dependants.lock().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<'a> Package<'a> {
    pub async fn run(&self, options: &Options) -> Result<()> {
        use std::process::Command;

        let mut cmd = Command::new("cargo");
        cmd.arg("publish");
        // cmd.args(options.args);
        if options.no_verify {
            cmd.arg("--no-verify");
        }
        cmd.current_dir(&self.path);
        if let Some(ref token) = options.registry_token {
            // std::env::set_var("CARGO_REGISTRY_TOKEN", token);
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
            return Ok(());
        }
        // wait for the package to become available here
        Ok(())
    }
}

// #[derive(Debug)]
// struct LocalDependency {
//     version: semver::VersionReq,
//     path: Option<PathBuf>,
// }

// this is the tool we want
// https://github.com/kjvalencik/actions/blob/master/run/index.ts
//manifest_path: impl AsRef<Path>
pub async fn test(options: &Options) -> Result<()> {
    // let options = options.as_ref();
    println!("searching cargo packages at {}", options.path.display());
    // actions::log_message(actions::LogLevel::Debug, "test");

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&options.path)
        .exec()?;
    // dbg!(&metadata);

    let packages = metadata.workspace_packages();
    // println!("included packages {}", []);
    // println!("excluded packages {}", []);

    // todo: filter the packages based on either cargo workspace metadata?
    // or arguments from action / cli
    // build dep index

    // for package in &packages {
    let publish_packages: HashMap<PathBuf, Package> = packages
        .into_iter()
        .filter_map(|package| {
            // dbg!(&package);
            // if package.name == "" {
            //     anyhow::bail!("{} has no name", &package.manifest_path);
            // }
            if let Some(publish) = &package.publish {
                // publish = ["some-registry-name"]
                // The value may also be an array of strings
                // which are registry names that are allowed to be published to.
                if publish.is_empty() {
                    // contains(&"false".to_string()) {
                    // skip package
                    // continue;
                    return None;
                }
            }
            if let Some(include) = &options.include {
                if include.len() > 0 {
                    if !include.contains(&package.name) {
                        // skip package
                        // continue;
                        return None;
                    }
                }
            }
            // let exclude = options.exclude.clone().unwrap_or_default();
            if let Some(exclude) = &options.exclude {
                if exclude.contains(&package.name) {
                    // skip package
                    // continue;
                    return None;
                }
            }
            // if package.version == "" {
            //     anyhow::bail!("{} has no name", &package.manifest_path);
            // }

            // break;
            // if package.package {
            // }
            // if package.workspaces.len() > 0 {
            //     println!("{} is workspace root", &package.name);
            // }
            // if is package:
            // if is workspace
            // resolve the versions even if they have none

            // dbg!(&package.name);
            // dbg!(&package.version);
            // dbg!(&package.dependencies);

            // check the dependencies in the next loop and resolve to if they use a path

            // build the index with this

            let path: PathBuf = package.manifest_path.parent().unwrap().into();
            Some((
                path.clone(),
                Package {
                    package,
                    path,
                    deps: Mutex::new(HashMap::new()),
                    dependants: Mutex::new(HashMap::new()),
                    // local_deps: Mutex::new(HashMap::new()),
                },
            ))
        })
        .collect();

    // let mut dependencies: HashMap<String, Dep> = HashMap::new();

    // for package in publish_packages.values_mut() {
    for p in publish_packages.values() {
        use toml_edit::{value, Document};
        let mut manifest =
            std::fs::read_to_string(&p.package.manifest_path)?.parse::<Document>()?;
        let mut need_update = false;

        for dep in &p.package.dependencies {
            dbg!(&dep.path);
            // dbg!(&dep.req);
            // dbg!(&dep.req == &semver::VersionReq::STAR);
            let mut dep_version = dep.req.clone();
            if let Some(path) = dep.path.as_ref().map(PathBuf::from) {
                // also if the version is set, we want to resolve automatically?
                // OR we allow chaning and always set allow-dirty
                // dep_version == semver::VersionReq::STAR &&
                let resolved = publish_packages.get(&path).ok_or(anyhow::anyhow!(
                    "could not resolve local dependency {}",
                    path.display()
                ))?;
                if options.resolve_versions {
                    // if let Some(resolved) =  {
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
                            manifest[kind][&dep.name]["version"] = value(dep_version.to_string());
                            manifest[kind][&dep.name]
                                .as_inline_table_mut()
                                .map(|t| t.fmt());
                        }
                    }
                    // }
                }

                // add local dependency
                p.deps
                    .lock()
                    .unwrap()
                    .insert(resolved.package.name.clone(), resolved);

                resolved
                    .dependants
                    .lock()
                    .unwrap()
                    .insert(p.package.name.clone(), p);
                // p.local_deps.lock().unwrap().insert(
                //     dep.name.clone(),
                //     LocalDependency {
                //         version: dep_version.clone(),
                //         path: dep.path.as_ref().map(PathBuf::from),
                //     },
                // );
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
    }

    // println!("packages: {:?}", &publish_packages);
    dbg!(&publish_packages);
    dbg!(&publish_packages.len());
    let package_names: Vec<String> = publish_packages
        .values()
        .map(|p| p.package.name.clone())
        .collect();
    println!("found packages: {:?}", package_names);

    // checking is not really necessary?
    // because we run --dry-run with cargo
    //  => does that check for versions already published
    //  => build and dep releated issues will be detected due to it packaging...
    // println!("checking package consistency");
    // todo

    // topological sort what to push first
    // let sorted = publish_packages.clone();
    // let sorted: HashMap<String, Package> = HashMap::new();

    // struct Graph
    // ready_nodes
    // for (id, p) in publish_packages.iter() {}
    //
    // scheduler loop here
    // find ready nodes
    if publish_packages.is_empty() {
        // we are done
        return Ok(());
    }
    let mut ready = publish_packages
        .iter()
        .filter(|(_, p)| p.deps.lock().unwrap().is_empty())
        .collect::<HashMap<_, _>>();
    if ready.is_empty() {
        // deadlock
        anyhow::bail!("deadlock");
    }
    use futures::{stream::FuturesUnordered, Future};
    let tasks: FuturesUnordered<Box<dyn Future<Output = ()>>> = FuturesUnordered::new();

    tasks.push(Box::new(async move {
        // return ourselves
        // ready
    }));

    //loop {
    //    //
    //}
    // Pin<Box<dyn Future<Output = PoolResult<(I, Result<O, E>)>> + Send + Sync>>>;
    // let

    // for package in sorted.values() {
    //     continue;
    // }
    Ok(())
}
