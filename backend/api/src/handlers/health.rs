use axum::{extract::State, Json};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

/// Liveness probe. Cheap, no I/O.
#[utoipa::path(
    get,
    path = "/health",
    tag = "system",
    responses((status = 200, description = "Service is up", body = Health))
)]
pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: "listenai-api",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadinessReport {
    pub status: &'static str,
    pub db: DbReadiness,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DbReadiness {
    pub reachable: bool,
    pub path: String,
}

/// Readiness probe. Verifies the DB connection answers a trivial query.
#[utoipa::path(
    get,
    path = "/ready",
    tag = "system",
    responses((status = 200, description = "Service is ready", body = ReadinessReport))
)]
pub async fn ready(State(state): State<AppState>) -> ApiResult<Json<ReadinessReport>> {
    // One-row ping into SurrealDB. Errors (including DB being down) are
    // intentionally swallowed into `reachable: false` so a readiness probe
    // never returns a 5xx just because the DB is degraded.
    let reachable = match state.db().inner().query("RETURN 1").await {
        Ok(mut r) => matches!(r.take::<Option<i64>>(0), Ok(Some(1))),
        Err(_) => false,
    };

    Ok(Json(ReadinessReport {
        status: if reachable { "ready" } else { "degraded" },
        db: DbReadiness {
            reachable,
            path: state.db().path().display().to_string(),
        },
    }))
}
