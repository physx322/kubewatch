//! KubeWatch updater: détection de nouvelles versions (GitHub Releases, registres OCI,
//! charts Helm), politique semver, déploiement et rollback.
#![forbid(unsafe_code)]

pub mod engine;
pub mod error;
pub mod github;
pub mod model;
pub mod policy;
pub mod rollout;
pub mod scan;
pub mod store;
pub mod webhook;

pub use engine::UpdateEngine;
pub use error::{Error, Result};
pub use github::GithubClient;
