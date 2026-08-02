#![forbid(unsafe_code)]

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response, sse::Event, sse::KeepAlive, sse::Sse},
    routing::{get, post},
};
use scg_core::{
    CONTRACT_VERSION, CommitReceipt, CommitRequest, CoreError, EventEnvelope, Runtime,
    RuntimeSnapshot,
};
use serde::Serialize;
use serde_json::json;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
struct ApiState {
    runtime: Arc<Runtime>,
}

pub fn router(runtime: Arc<Runtime>) -> Router {
    Router::new()
        .route("/healthz", get(liveness))
        .route("/readyz", get(readiness))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/events", get(events))
        .route("/v1/runtime/start", post(start))
        .route("/v1/runtime/stop", post(stop))
        .route("/v1/state/commit", post(commit))
        .with_state(ApiState { runtime })
        .layer(TraceLayer::new_for_http())
}

async fn liveness() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "contractVersion": CONTRACT_VERSION,
    }))
}

async fn readiness(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let health = state.runtime.health()?;
    let status = if health.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    Ok((status, Json(health)).into_response())
}

async fn snapshot(State(state): State<ApiState>) -> Result<Json<RuntimeSnapshot>, ApiError> {
    Ok(Json(state.runtime.snapshot()?))
}

async fn start(State(state): State<ApiState>) -> Result<Json<RuntimeSnapshot>, ApiError> {
    Ok(Json(state.runtime.start()?))
}

async fn stop(State(state): State<ApiState>) -> Result<Json<RuntimeSnapshot>, ApiError> {
    Ok(Json(state.runtime.stop()?))
}

async fn commit(
    State(state): State<ApiState>,
    Json(request): Json<CommitRequest>,
) -> Result<Json<CommitReceipt>, ApiError> {
    Ok(Json(state.runtime.commit(request)?))
}

async fn events(
    State(state): State<ApiState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.runtime.subscribe())
        .filter_map(|result| result.ok().and_then(to_sse_event).map(Ok));
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn to_sse_event(envelope: EventEnvelope) -> Option<Event> {
    Event::default()
        .id(envelope.sequence.to_string())
        .event(envelope.kind.clone())
        .json_data(envelope)
        .ok()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    code: &'static str,
    message: String,
}

struct ApiError(CoreError);

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            CoreError::RevisionConflict { .. } => (StatusCode::CONFLICT, "revision_conflict"),
            CoreError::InvalidTransition { .. } => (StatusCode::CONFLICT, "invalid_transition"),
            CoreError::InvalidRealm(_)
            | CoreError::InvalidServiceGraph(_)
            | CoreError::InvalidOperation(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            CoreError::RealmMismatch { .. } => (StatusCode::CONFLICT, "realm_mismatch"),
            CoreError::ChecksumMismatch => (StatusCode::SERVICE_UNAVAILABLE, "integrity_failure"),
            CoreError::LockPoisoned | CoreError::Storage(_) | CoreError::Serialization(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (
            status,
            Json(ErrorBody {
                code,
                message: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::router;
    use scg_core::{RealmId, Runtime, RuntimeConfig};

    #[tokio::test]
    async fn health_and_snapshot_routes_are_available() {
        let directory = tempdir().unwrap();
        let runtime = Arc::new(
            Runtime::open(RuntimeConfig::new(
                RealmId::Live,
                directory.path().join("node.db"),
            ))
            .unwrap(),
        );
        runtime.start().unwrap();
        let app = router(runtime);

        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let snapshot = app
            .oneshot(
                Request::builder()
                    .uri("/v1/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn commits_return_receipts_and_reject_stale_revisions() {
        let directory = tempdir().unwrap();
        let runtime = Arc::new(
            Runtime::open(RuntimeConfig::new(
                RealmId::Live,
                directory.path().join("node.db"),
            ))
            .unwrap(),
        );
        runtime.start().unwrap();
        let app = router(runtime);
        let request_body = r#"{
            "expectedRevision": 0,
            "operation": "capacity.updated",
            "state": {"workerLimit": 8}
        }"#;

        let committed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/state/commit")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(committed.status(), StatusCode::OK);

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/state/commit")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }
}
