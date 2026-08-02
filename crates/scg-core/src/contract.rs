use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RealmId;

pub const CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimePhase {
    Stopped,
    Starting,
    Ready,
    Degraded,
    Stopping,
    Failed,
}

impl std::fmt::Display for RuntimePhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSnapshot {
    pub id: String,
    pub status: ServiceStatus,
    pub start_order: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub contract_version: u16,
    pub realm: RealmId,
    pub phase: RuntimePhase,
    pub revision: u64,
    pub clean_shutdown: bool,
    pub services: Vec<ServiceSnapshot>,
    pub last_commit_at: Option<String>,
    pub state: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub contract_version: u16,
    pub ready: bool,
    pub phase: RuntimePhase,
    pub revision: u64,
    pub realm: RealmId,
    pub storage_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub expected_revision: u64,
    pub operation: String,
    pub state: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitReceipt {
    pub revision: u64,
    pub event_sequence: u64,
    pub checksum: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub contract_version: u16,
    pub sequence: u64,
    pub revision: u64,
    pub kind: String,
    pub occurred_at: String,
    pub payload: Value,
}
