//! Publishing state of workspace packages and their discovery from Cargo metadata.

use crate::Options;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Tracks publishing state and local dependency edges for a Cargo package.
pub(crate) struct Package {
    pub(crate) inner: cargo_metadata::Package,
    pub(crate) path: PathBuf,
    pub(crate) publishable: bool,
    pub(crate) should_publish: bool,
    pub(crate) published: Mutex<bool>,
    pub(crate) deps: RwLock<HashMap<String, Arc<Package>>>,
    pub(crate) dependants: RwLock<HashMap<String, Arc<Package>>>,
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
}

pub(crate) fn find_packages(
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

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq as sim_assert_eq;

    use crate::test_support::{options, package_map, write_member};

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
}
