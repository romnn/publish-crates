//! Workspace fixtures shared by the unit tests.

use std::path::{Path, PathBuf};

pub(crate) fn options(path: PathBuf) -> crate::Options {
    crate::Options {
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

pub(crate) fn write_member(workspace: &Path, name: &str, manifest_suffix: &str) {
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

pub(crate) fn package_map(
    metadata: &cargo_metadata::Metadata,
    options: &crate::Options,
) -> std::collections::HashMap<PathBuf, std::sync::Arc<crate::package::Package>> {
    crate::package::find_packages(metadata, options).collect()
}

/// Builds a workspace where `consumer` depends on its sibling `foo` and returns `consumer`.
#[cfg(unix)]
pub(crate) async fn consumer_of_local_foo(
    workspace: &Path,
) -> std::sync::Arc<crate::package::Package> {
    let workspace_manifest_path = workspace.join("Cargo.toml");
    std::fs::write(
        &workspace_manifest_path,
        r#"[workspace]
members = ["crates/foo", "crates/consumer"]
resolver = "2"
"#,
    )
    .expect("workspace manifest must be written");
    write_member(workspace, "foo", "");
    write_member(
        workspace,
        "consumer",
        r#"[dependencies]
foo = { path = "../foo" }
"#,
    );

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&workspace_manifest_path)
        .exec()
        .expect("workspace metadata must load");
    let mut options = options(workspace_manifest_path);
    options.resolve_versions = true;
    let packages = package_map(&metadata, &options);
    crate::graph::build_dag(&metadata, &packages, &options)
        .await
        .expect("dependency graph must resolve");

    packages
        .into_values()
        .find(|package| package.inner.name.as_str() == "consumer")
        .expect("consumer package must be discovered")
}
