//! Lookup and editing of dependency entries in a package manifest.

use cargo_metadata::DependencyKind;
use cargo_metadata::cargo_platform::Platform;
use color_eyre::eyre;

pub(crate) fn update_dependency_version(
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

pub(crate) struct DependencyDeclaration {
    pub(crate) has_version: bool,
    pub(crate) inherits_workspace: bool,
}

pub(crate) fn dependency_declaration(
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
