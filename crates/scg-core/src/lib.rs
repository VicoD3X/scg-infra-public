#![forbid(unsafe_code)]

mod contract;
mod error;
mod realm;
mod runtime;
mod service;
mod store;

pub use contract::{
    CONTRACT_VERSION, CommitReceipt, CommitRequest, EventEnvelope, HealthSnapshot, RuntimePhase,
    RuntimeSnapshot, ServiceSnapshot, ServiceStatus,
};
pub use error::{CoreError, CoreResult};
pub use realm::RealmId;
pub use runtime::{Runtime, RuntimeConfig};
pub use service::{ServiceGraph, ServiceSpec};
