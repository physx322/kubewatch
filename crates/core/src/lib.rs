//! KubeWatch core: connexion cluster, découverte, CRUD générique, apply YAML,
//! logs, exec, port-forward, métriques et évènements.
#![forbid(unsafe_code)]

pub mod apply;
pub mod client;
pub mod cluster;
pub mod discovery;
pub mod error;
pub mod events;
pub mod exec;
pub mod logs;
pub mod metrics;
pub mod model;
pub mod portforward;
pub mod resource;

pub use client::{connect, ConnectionSpec};
pub use cluster::{ClusterHandle, ClusterManager};
pub use error::{Error, Result};

pub use kube;
pub use k8s_openapi;
