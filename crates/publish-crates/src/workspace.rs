//! Version resolution for local packages in `[workspace.dependencies]`.

use crate::Options;
use crate::package::Package;
use action_core as action;
use cargo_metadata::DependencyKind;
use color_eyre::eyre;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Updates local path dependency versions in `[workspace.dependencies]`.
///
/// This is required for newer Cargo versions which enforce that any
/// published dependency that has a `path` also specifies an explicit
/// version requirement. For workspaces that rely on `[workspace.dependencies]`
/// plus `foo.workspace = true` in member manifests, the version must be set
/// on the workspace-level dependency.
pub(crate) async fn update_workspace_dependencies(
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

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq as sim_assert_eq;

    use crate::test_support::{options, package_map, write_member};

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
}
