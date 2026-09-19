//! KubeWatch hub: recherche et déploiement d'images de conteneurs et de charts Helm.
#![forbid(unsafe_code)]

pub mod catalog;
pub mod charts;
pub mod deploy;
pub mod error;
pub mod model;
pub mod registry;
pub mod sizing;

pub use error::{Error, Result};
pub use registry::HubClient;
