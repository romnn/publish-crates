//! Concurrent publishing of the selected packages in dependency order.

use crate::Options;
use crate::graph::build_dag;
use crate::package::{Package, find_packages};
use crate::workspace::update_workspace_dependencies;
use action_core as action;
use color_eyre::eyre;
use futures::Future;
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;

type TaskFut = dyn Future<Output = eyre::Result<Arc<Package>>>;

/// Publishes selected workspace packages to crates.io in dependency order.
///
/// Local path dependencies form the publishing graph. Independent packages run concurrently up to
/// [`Options::concurrency_limit`], while each dependant waits for its dependencies to appear on the
/// registry.
///
/// Versionless development dependencies on private packages are excluded because Cargo omits them
/// from the published manifest.
///
/// When the registry's `min-publish-age` rejects the release of a local dependency as too new, the
/// package is published again with `CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow`.
/// Cargo applies that to the whole invocation, including registry dependencies that the workspace
/// lockfile does not pin.
/// Setting that variable in the process environment, such as to `deny`, disables the exemption.
///
/// # Errors
///
/// Returns an error when the options or Cargo metadata are invalid, dependency versions cannot be
/// resolved, a selected package depends on an excluded package, `cargo publish` fails permanently,
/// or a published package does not become available before the registry timeout.
/// A dependency that `min-publish-age` rejects fails at once, unless the exemption above applies.
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
                eyre::bail!("a task failed: {err}");
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
