//! Validation of local dependencies and construction of the publishing graph.

use crate::Options;
use crate::manifest::{dependency_declaration, update_dependency_version};
use crate::package::Package;
use action_core as action;
use cargo_metadata::DependencyKind;
use color_eyre::{Section, eyre};
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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

pub(crate) async fn build_dag(
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

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq as sim_assert_eq;

    use crate::test_support::{options, package_map, write_member};

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

        let changed =
            crate::workspace::update_workspace_dependencies(&metadata, &packages, &options)
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
}
