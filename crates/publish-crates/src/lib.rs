//! Publishes interdependent Cargo workspace packages in dependency order.
//!
//! The crate discovers workspace members with Cargo metadata, resolves local dependency order, and
//! runs `cargo publish` concurrently for packages whose dependencies are available.
//!
//! # Examples
//!
//! ```no_run
//! # // Uses `no_run` because publishing requires a Cargo workspace and registry access.
//! # async fn example() -> color_eyre::eyre::Result<()> {
//! use publish_crates::{Options, publish};
//! use std::path::PathBuf;
//!
//! publish(Options {
//!     path: PathBuf::from("Cargo.toml"),
//!     registry_token: None,
//!     dry_run: true,
//!     publish_delay: None,
//!     no_verify: false,
//!     resolve_versions: false,
//!     include: None,
//!     exclude: None,
//!     max_retries: None,
//!     concurrency_limit: Some(4),
//!     extra_args: Vec::new(),
//! })
//! .await?;
//! # Ok(())
//! # }
//! ```

mod cargo_publish;
mod diagnostics;
mod graph;
mod manifest;
mod options;
mod package;
mod registry;
mod scheduler;
#[cfg(test)]
mod test_support;
mod workspace;

pub use diagnostics::PublishError;
pub use options::Options;
pub use scheduler::publish;
