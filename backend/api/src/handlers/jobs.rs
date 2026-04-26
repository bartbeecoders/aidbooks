//! Read-only job inspection endpoints — a polling fallback for clients that
//! can't (or don't want to) hold a WebSocket open.

use axum::{
    extract::{Path, State},
    Json,
};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use listenai_jobs::hub::JobSnapshot;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookJobList {
    pub audiobook_id: String,
    pub jobs: Vec<JobSnapshot>,
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}/jobs",
    tag = "jobs",
    params(("id" = String, Path, description = "Audiobook id")),
    responses(
        (status = 200, description = "Latest status of every job for this audiobook", body = AudiobookJobList),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn list_for_audiobook(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<AudiobookJobList>> {
    assert_owner(&state, &id, &user.id).await?;
    let jobs = state
        .jobs()
        .list_for_audiobook(&id)
        .await?
        .into_iter()
        .map(|j| JobSnapshot {
            id: j.id,
            kind: j.kind.as_str().to_string(),
            status: j.status.as_str().to_string(),
            progress_pct: j.progress_pct,
            attempts: j.attempts,
            chapter_number: j.chapter_number,
            last_error: j.last_error,
        })
        .collect();
    Ok(Json(AudiobookJobList {
        audiobook_id: id,
        jobs,
    }))
}

async fn assert_owner(state: &AppState, audiobook_id: &str, user: &UserId) -> Result<()> {
    #[derive(Deserialize)]
    struct Row {
        owner: surrealdb::sql::Thing,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT owner FROM audiobook:`{audiobook_id}`"))
        .await
        .map_err(|e| Error::Database(format!("jobs owner: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("jobs owner (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id}"),
    })?;
    if row.owner.id.to_raw() != user.0 {
        return Err(Error::NotFound {
            resource: format!("audiobook:{audiobook_id}"),
        });
    }
    Ok(())
}
