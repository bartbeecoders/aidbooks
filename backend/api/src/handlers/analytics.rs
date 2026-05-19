//! Owner-scoped analytics dashboard.
//!
//! Four endpoints, each returning a JSON shape directly consumable by
//! the frontend's chart components — no chart-library-specific
//! formatting on this side.
//!
//!   * `GET /analytics/generation` — local-DB rollup of how many
//!     audiobooks, shorts, and YouTube-published videos the calling
//!     user produced, bucketed by day / week / month, alongside the
//!     total narration duration and the cumulative LLM/TTS spend
//!     captured in `generation_event`.
//!   * `GET /analytics/youtube/channel` — the connected channel's
//!     lifetime statistics (subscriber count, total views, video
//!     count).
//!   * `GET /analytics/youtube/videos` — per-video views/likes/
//!     comments for every video this user has published through the
//!     app, joined with the originating audiobook so the table is
//!     identifiable.
//!   * `GET /analytics/youtube/reports` — day-bucketed YouTube
//!     Analytics API report (views, likes, comments, watch-time
//!     minutes), re-bucketed to week / month server-side when the
//!     caller asks for a coarser grain.
//!
//! Buckets are computed in Rust off the row's `created_at` /
//! `published_at` because SurrealDB's `time::format` doesn't compose
//! cleanly with `GROUP BY` for ISO-week math, and we want week
//! starts to land consistently on Mondays.

use std::collections::BTreeMap;
use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, Utc};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;
use crate::youtube::{
    account::{access_token, drop_account},
    analytics as yt_analytics,
};

// ---------------- Common ---------------------------------------------------

/// Bucket granularity for time-series rollups. `Day` is the natural
/// resolution most metrics arrive at; `Week` (ISO Monday-start) and
/// `Month` are derived by collapsing day buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Day,
    Week,
    Month,
}

impl Bucket {
    fn parse(s: Option<&str>) -> Result<Self> {
        Ok(match s.unwrap_or("day").trim().to_ascii_lowercase().as_str() {
            "day" => Bucket::Day,
            "week" => Bucket::Week,
            "month" => Bucket::Month,
            other => {
                return Err(Error::Validation(format!(
                    "bucket must be day/week/month, got `{other}`"
                )))
            }
        })
    }

    /// ISO-style key for a given UTC instant. Week is `YYYY-Www` (ISO
    /// week, Monday-anchored); month is `YYYY-MM`; day is `YYYY-MM-DD`.
    /// The key doubles as the date the chart displays.
    fn key(self, dt: DateTime<Utc>) -> String {
        let d = dt.date_naive();
        match self {
            Bucket::Day => d.format("%Y-%m-%d").to_string(),
            Bucket::Week => {
                // Anchor weeks to the ISO Monday so the same key
                // covers Mon–Sun regardless of the row's weekday.
                let iso = d.iso_week();
                format!("{:04}-W{:02}", iso.year(), iso.week())
            }
            Bucket::Month => d.format("%Y-%m").to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsQuery {
    /// `day` (default), `week`, or `month`.
    #[serde(default)]
    pub bucket: Option<String>,
    /// How far back to look, in days. Clamped to `[1..=365*3]`. Defaults
    /// to 30 days for day-bucketing and is automatically extended for
    /// coarser buckets so the chart isn't empty (`week` → ≥90, `month`
    /// → ≥365).
    #[serde(default)]
    pub range_days: Option<u32>,
}

impl AnalyticsQuery {
    fn parsed(&self) -> Result<(Bucket, DateTime<Utc>)> {
        let bucket = Bucket::parse(self.bucket.as_deref())?;
        let default_days = match bucket {
            Bucket::Day => 30,
            Bucket::Week => 90,
            Bucket::Month => 365,
        };
        let n = self
            .range_days
            .map(|n| n.clamp(1, 365 * 3))
            .unwrap_or(default_days);
        let since = Utc::now() - ChronoDuration::days(n as i64);
        Ok((bucket, since))
    }
}

// ---------------- /analytics/generation ------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct GenerationPoint {
    /// Bucket key — `YYYY-MM-DD`, `YYYY-Www`, or `YYYY-MM`.
    pub date: String,
    /// Non-short audiobooks created in this bucket.
    pub audiobooks_count: u32,
    /// Sum of `audiobook.duration_ms` for those rows. `0` when the
    /// books in the bucket haven't been narrated yet.
    pub audiobooks_duration_ms: u64,
    /// Sum of `generation_event.cost_usd` charged for events whose
    /// audiobook is non-short and was created in this bucket.
    pub audiobooks_cost_usd: f64,
    /// Short-form audiobooks (`is_short = true`) created in this bucket.
    pub shorts_count: u32,
    pub shorts_duration_ms: u64,
    pub shorts_cost_usd: f64,
    /// YouTube publications that successfully published in this
    /// bucket (`published_at` set).
    pub videos_count: u32,
    /// Sum of `audiobook.duration_ms` for the audiobooks behind those
    /// videos. The video itself shares the audiobook's runtime.
    pub videos_duration_ms: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GenerationSeries {
    /// Echo back the actual bucket used so the frontend doesn't have
    /// to re-parse its own query string.
    pub bucket: String,
    pub range_days: u32,
    pub points: Vec<GenerationPoint>,
}

#[utoipa::path(
    get,
    path = "/analytics/generation",
    tag = "analytics",
    params(
        ("bucket" = Option<String>, Query, description = "day | week | month (default: day)"),
        ("range_days" = Option<u32>, Query, description = "Lookback window in days; auto-extends for week/month if absent")
    ),
    responses(
        (status = 200, description = "Time-series counts/durations/costs", body = GenerationSeries),
        (status = 400, description = "Bad bucket value"),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn generation(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Query(q): Query<AnalyticsQuery>,
) -> ApiResult<Json<GenerationSeries>> {
    let (bucket, since) = q.parsed()?;
    let range_days = (Utc::now() - since).num_days().max(1) as u32;

    // --- Audiobooks (+ duration) ----------------------------------------
    #[derive(Debug, Deserialize)]
    struct AudiobookRow {
        id: Thing,
        created_at: DateTime<Utc>,
        #[serde(default)]
        is_short: Option<bool>,
        #[serde(default)]
        duration_ms: Option<i64>,
    }
    let audiobooks: Vec<AudiobookRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, created_at, is_short, duration_ms FROM audiobook \
             WHERE owner = user:`{uid}` AND created_at >= $since",
            uid = user.id.0
        ))
        .bind(("since", since))
        .await
        .map_err(|e| Error::Database(format!("analytics audiobooks: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("analytics audiobooks (decode): {e}")))?;

    // Quick lookup: audiobook id → (bucket key, is_short). The
    // generation-event pass below uses this to attribute cost back to
    // the bucket the source audiobook lives in, which is the only
    // sane interpretation for events that arrive minutes/hours after
    // creation.
    let mut audiobook_meta: HashMap<String, (String, bool)> = HashMap::new();
    let mut points: BTreeMap<String, GenerationPoint> = BTreeMap::new();
    for r in &audiobooks {
        let key = bucket.key(r.created_at);
        let is_short = r.is_short.unwrap_or(false);
        audiobook_meta.insert(r.id.id.to_raw(), (key.clone(), is_short));
        let p = points.entry(key.clone()).or_insert_with(|| empty_point(&key));
        let duration = r.duration_ms.unwrap_or(0).max(0) as u64;
        if is_short {
            p.shorts_count += 1;
            p.shorts_duration_ms += duration;
        } else {
            p.audiobooks_count += 1;
            p.audiobooks_duration_ms += duration;
        }
    }

    // --- Costs ---------------------------------------------------------
    #[derive(Debug, Deserialize)]
    struct EventRow {
        #[serde(default)]
        audiobook: Option<Thing>,
        #[serde(default)]
        cost_usd: f64,
    }
    let events: Vec<EventRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audiobook, cost_usd FROM generation_event \
             WHERE user = user:`{uid}` AND created_at >= $since",
            uid = user.id.0
        ))
        .bind(("since", since))
        .await
        .map_err(|e| Error::Database(format!("analytics events: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("analytics events (decode): {e}")))?;
    for e in events {
        let Some(book_thing) = e.audiobook else {
            // Events without an audiobook FK (random_topic, moderation)
            // aren't surfaced in the per-content-type breakdown — they
            // wouldn't have anywhere meaningful to land.
            continue;
        };
        let Some((key, is_short)) = audiobook_meta.get(&book_thing.id.to_raw()) else {
            // Cost for an audiobook outside the window or owned by
            // someone else. The owner-scoped event filter already
            // excluded the latter, so this is just the window edge.
            continue;
        };
        let p = points
            .entry(key.clone())
            .or_insert_with(|| empty_point(key));
        if *is_short {
            p.shorts_cost_usd += e.cost_usd;
        } else {
            p.audiobooks_cost_usd += e.cost_usd;
        }
    }

    // --- Published videos ---------------------------------------------
    #[derive(Debug, Deserialize)]
    struct PubRow {
        audiobook: Thing,
        published_at: DateTime<Utc>,
    }
    // Two-step: load this user's audiobooks' publications (single-mode
    // videos) plus playlist-mode per-chapter videos. We filter on
    // `audiobook.owner` so a shared row never bleeds into another
    // user's totals.
    let single_pubs: Vec<PubRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audiobook, published_at FROM youtube_publication \
             WHERE audiobook.owner = user:`{uid}` \
               AND published_at != NONE AND published_at >= $since",
            uid = user.id.0
        ))
        .bind(("since", since))
        .await
        .map_err(|e| Error::Database(format!("analytics pubs: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("analytics pubs (decode): {e}")))?;
    // Playlist mode: every chapter is its own video. We attribute one
    // "video" per chapter row that landed in the window.
    #[derive(Debug, Deserialize)]
    struct PlaylistVideoRow {
        audiobook: Thing,
        published_at: DateTime<Utc>,
    }
    let playlist_pubs: Vec<PlaylistVideoRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT publication.audiobook AS audiobook, published_at \
             FROM youtube_publication_video \
             WHERE publication.audiobook.owner = user:`{uid}` \
               AND published_at != NONE AND published_at >= $since",
            uid = user.id.0
        ))
        .bind(("since", since))
        .await
        .map_err(|e| Error::Database(format!("analytics playlist pubs: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("analytics playlist pubs (decode): {e}")))?;

    // Pull durations once for every audiobook referenced from a
    // publication. The audiobook table holds the canonical narration
    // total — the publication doesn't re-store it.
    #[derive(Debug, Deserialize)]
    struct DurRow {
        id: Thing,
        #[serde(default)]
        duration_ms: Option<i64>,
    }
    let mut audiobook_durations: HashMap<String, u64> = HashMap::new();
    let referenced_books: Vec<String> = single_pubs
        .iter()
        .map(|p| p.audiobook.id.to_raw())
        .chain(playlist_pubs.iter().map(|p| p.audiobook.id.to_raw()))
        .collect();
    if !referenced_books.is_empty() {
        let dur_rows: Vec<DurRow> = state
            .db()
            .inner()
            .query(
                "SELECT id, duration_ms FROM audiobook \
                 WHERE record::id(id) INSIDE $ids",
            )
            .bind(("ids", referenced_books))
            .await
            .map_err(|e| Error::Database(format!("analytics durations: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("analytics durations (decode): {e}")))?;
        for d in dur_rows {
            audiobook_durations.insert(d.id.id.to_raw(), d.duration_ms.unwrap_or(0).max(0) as u64);
        }
    }

    for p in single_pubs.iter().map(|p| (&p.audiobook, p.published_at)).chain(
        playlist_pubs
            .iter()
            .map(|p| (&p.audiobook, p.published_at)),
    ) {
        let (book, ts) = p;
        let key = bucket.key(ts);
        let entry = points.entry(key.clone()).or_insert_with(|| empty_point(&key));
        entry.videos_count += 1;
        entry.videos_duration_ms += audiobook_durations
            .get(&book.id.to_raw())
            .copied()
            .unwrap_or(0);
    }

    // --- Fill empty buckets so the chart line stays continuous --------
    fill_empty_points(&mut points, bucket, since);

    Ok(Json(GenerationSeries {
        bucket: match bucket {
            Bucket::Day => "day",
            Bucket::Week => "week",
            Bucket::Month => "month",
        }
        .to_string(),
        range_days,
        points: points.into_values().collect(),
    }))
}

fn empty_point(date: &str) -> GenerationPoint {
    GenerationPoint {
        date: date.to_string(),
        audiobooks_count: 0,
        audiobooks_duration_ms: 0,
        audiobooks_cost_usd: 0.0,
        shorts_count: 0,
        shorts_duration_ms: 0,
        shorts_cost_usd: 0.0,
        videos_count: 0,
        videos_duration_ms: 0,
    }
}

/// Walk forward from `since` to today inserting zero-valued points for
/// any bucket that didn't see activity. The chart is much easier to
/// read when the x-axis is a contiguous timeline than when gap days
/// are simply missing from the series.
fn fill_empty_points(
    points: &mut BTreeMap<String, GenerationPoint>,
    bucket: Bucket,
    since: DateTime<Utc>,
) {
    let today = Utc::now();
    let mut cursor = since.date_naive();
    let end = today.date_naive();
    while cursor <= end {
        // Anchor at noon UTC so DST/timezone wobble can't push the
        // resulting key into the wrong day bucket. `and_hms_opt(12,0,0)`
        // is total over valid `NaiveDate`, but match avoids the
        // `unwrap_used` lint without losing intent.
        let anchored = match cursor.and_hms_opt(12, 0, 0) {
            Some(t) => t.and_utc(),
            None => break,
        };
        let key = bucket.key(anchored);
        points.entry(key.clone()).or_insert_with(|| empty_point(&key));
        cursor = match bucket {
            Bucket::Day => cursor + ChronoDuration::days(1),
            Bucket::Week => cursor + ChronoDuration::days(7),
            // Month-step: cheap walk by adding 28 days, the bucket
            // dedupes for us so accidental same-month revisits are
            // harmless.
            Bucket::Month => cursor + ChronoDuration::days(28),
        };
        // Guard against infinite loop if NaiveDate addition saturates
        // (won't realistically happen but a misconfigured range_days
        // shouldn't be able to hang the API).
        if cursor < since.date_naive() {
            break;
        }
        let _ = NaiveDate::from_ymd_opt(cursor.year(), cursor.month(), cursor.day());
    }
}

// ---------------- /analytics/youtube/channel ------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeChannelSummary {
    pub channel_id: String,
    pub channel_title: String,
    /// Lifetime subscriber count. YouTube rounds publicly; the API
    /// returns the same rounded value unless the channel hides
    /// subscriber count, in which case this is `0`.
    pub subscriber_count: u64,
    /// Lifetime public-video view count across the channel.
    pub view_count: u64,
    /// Number of public videos on the channel.
    pub video_count: u64,
}

#[utoipa::path(
    get,
    path = "/analytics/youtube/channel",
    tag = "analytics",
    responses(
        (status = 200, description = "Connected channel's lifetime stats", body = YoutubeChannelSummary),
        (status = 401, description = "Unauthenticated, or YouTube grant was revoked at Google"),
        (status = 409, description = "User has not connected a YouTube channel")
    ),
    security(("bearer" = []))
)]
pub async fn youtube_channel(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<YoutubeChannelSummary>> {
    let token = require_yt_token(&state, &user.id).await?;
    let stats = match yt_analytics::fetch_channel_stats(&token).await {
        Ok(s) => s,
        Err(Error::Unauthorized) => {
            // Refresh was fine but the call itself bounced — most
            // likely a scope mismatch (older grants don't include the
            // readonly scopes). Drop the row so the next request asks
            // the user to reconnect with the broader consent screen.
            let _ = drop_account(&state, &user.id).await;
            return Err(Error::Unauthorized.into());
        }
        Err(e) => return Err(e.into()),
    };
    Ok(Json(YoutubeChannelSummary {
        channel_id: stats.channel_id,
        channel_title: stats.channel_title,
        subscriber_count: stats.subscriber_count,
        view_count: stats.view_count,
        video_count: stats.video_count,
    }))
}

// ---------------- /analytics/youtube/videos -------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeVideoRow {
    pub video_id: String,
    /// Audiobook this video was published from. Always present
    /// because we only walk publication rows owned by this user.
    pub audiobook_id: String,
    pub audiobook_title: String,
    /// 1-based chapter number for playlist-mode publications.
    /// `None` for single-mode videos.
    pub chapter_number: Option<i64>,
    pub published_at: Option<DateTime<Utc>>,
    pub view_count: u64,
    pub like_count: u64,
    pub comment_count: u64,
    /// `true` when the source audiobook was generated as a YouTube
    /// Short (one chapter, ≤ 90 s narration). Mutually exclusive with
    /// `is_songbook` — drives the grouping on the analytics page.
    pub is_short: bool,
    /// `true` when the source audiobook was generated from a song
    /// (lyrics-driven outline + snippet splicing).
    pub is_songbook: bool,
    /// Per-video share of the source audiobook's total generation cost:
    /// audiobook cost divided by the number of videos published from
    /// that audiobook. Summing this column equals the sum of each
    /// audiobook's full cost (no double-counting on playlist mode).
    pub cost_usd: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeVideoList {
    pub items: Vec<YoutubeVideoRow>,
    /// Total across the visible list — handy for the dashboard tile
    /// without making the frontend re-sum.
    pub total_views: u64,
    pub total_likes: u64,
    pub total_comments: u64,
    /// Total generation cost across all audiobooks that produced a
    /// video in this list (each audiobook counted once).
    pub total_cost_usd: f64,
}

#[utoipa::path(
    get,
    path = "/analytics/youtube/videos",
    tag = "analytics",
    responses(
        (status = 200, description = "Per-video YouTube stats joined with the source audiobook", body = YoutubeVideoList),
        (status = 401, description = "Unauthenticated, or YouTube grant was revoked at Google"),
        (status = 409, description = "User has not connected a YouTube channel")
    ),
    security(("bearer" = []))
)]
pub async fn youtube_videos(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<YoutubeVideoList>> {
    let token = require_yt_token(&state, &user.id).await?;

    // Collect every video id this user has published, joined with the
    // source audiobook so the table is identifiable. Two queries —
    // single-mode publications + playlist-mode per-chapter rows.
    #[derive(Debug, Deserialize)]
    struct SinglePub {
        audiobook: Thing,
        #[serde(default)]
        video_id: Option<String>,
        #[serde(default)]
        published_at: Option<DateTime<Utc>>,
    }
    let single: Vec<SinglePub> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audiobook, video_id, published_at FROM youtube_publication \
             WHERE audiobook.owner = user:`{uid}` AND video_id != NONE",
            uid = user.id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt videos single: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt videos single (decode): {e}")))?;

    #[derive(Debug, Deserialize)]
    struct PlaylistVideo {
        audiobook: Thing,
        chapter_number: i64,
        #[serde(default)]
        video_id: Option<String>,
        #[serde(default)]
        published_at: Option<DateTime<Utc>>,
    }
    let playlist: Vec<PlaylistVideo> = state
        .db()
        .inner()
        .query(format!(
            "SELECT publication.audiobook AS audiobook, chapter_number, video_id, published_at \
             FROM youtube_publication_video \
             WHERE publication.audiobook.owner = user:`{uid}` AND video_id != NONE",
            uid = user.id.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt videos playlist: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt videos playlist (decode): {e}")))?;

    // Pull audiobook title + the type flags the analytics page groups
    // on (`is_short`, `is_songbook`). Older rows that pre-date the
    // songbook/short migrations may have these fields absent — default
    // to false so they fall into the plain "Audiobooks" bucket.
    #[derive(Debug, Deserialize)]
    struct BookRow {
        id: Thing,
        title: String,
        #[serde(default)]
        is_short: Option<bool>,
        #[serde(default)]
        is_songbook: Option<bool>,
    }
    let book_ids: Vec<String> = single
        .iter()
        .map(|s| s.audiobook.id.to_raw())
        .chain(playlist.iter().map(|p| p.audiobook.id.to_raw()))
        .collect();
    struct BookMeta {
        title: String,
        is_short: bool,
        is_songbook: bool,
    }
    let mut books: HashMap<String, BookMeta> = HashMap::new();
    if !book_ids.is_empty() {
        let rows: Vec<BookRow> = state
            .db()
            .inner()
            .query(
                "SELECT id, title, is_short, is_songbook FROM audiobook \
                 WHERE record::id(id) INSIDE $ids",
            )
            .bind(("ids", book_ids.clone()))
            .await
            .map_err(|e| Error::Database(format!("yt videos titles: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("yt videos titles (decode): {e}")))?;
        for r in rows {
            books.insert(
                r.id.id.to_raw(),
                BookMeta {
                    title: r.title,
                    is_short: r.is_short.unwrap_or(false),
                    is_songbook: r.is_songbook.unwrap_or(false),
                },
            );
        }
    }

    // Total generation_event cost per audiobook for the books in this
    // result. We sum once per book and (further down) split that total
    // across the videos published from it so playlist-mode rows don't
    // double-count the cost in the table footer.
    #[derive(Debug, Deserialize)]
    struct CostRow {
        audiobook: Thing,
        #[serde(default)]
        cost_usd: f64,
    }
    let mut book_cost: HashMap<String, f64> = HashMap::new();
    if !book_ids.is_empty() {
        let rows: Vec<CostRow> = state
            .db()
            .inner()
            .query(
                "SELECT audiobook, cost_usd FROM generation_event \
                 WHERE record::id(audiobook) INSIDE $ids",
            )
            .bind(("ids", book_ids))
            .await
            .map_err(|e| Error::Database(format!("yt videos costs: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("yt videos costs (decode): {e}")))?;
        for r in rows {
            *book_cost.entry(r.audiobook.id.to_raw()).or_insert(0.0) += r.cost_usd;
        }
    }

    // Build the (video_id → audiobook+chapter) lookup we'll merge the
    // YouTube stats response into.
    struct Meta {
        audiobook_id: String,
        audiobook_title: String,
        chapter_number: Option<i64>,
        published_at: Option<DateTime<Utc>>,
        is_short: bool,
        is_songbook: bool,
    }
    let mut meta: HashMap<String, Meta> = HashMap::new();
    for s in single {
        let Some(vid) = s.video_id else { continue };
        let book_id = s.audiobook.id.to_raw();
        let book = books.get(&book_id);
        meta.insert(
            vid,
            Meta {
                audiobook_id: book_id,
                audiobook_title: book.map(|b| b.title.clone()).unwrap_or_default(),
                chapter_number: None,
                published_at: s.published_at,
                is_short: book.map(|b| b.is_short).unwrap_or(false),
                is_songbook: book.map(|b| b.is_songbook).unwrap_or(false),
            },
        );
    }
    for p in playlist {
        let Some(vid) = p.video_id else { continue };
        let book_id = p.audiobook.id.to_raw();
        let book = books.get(&book_id);
        meta.insert(
            vid,
            Meta {
                audiobook_id: book_id,
                audiobook_title: book.map(|b| b.title.clone()).unwrap_or_default(),
                chapter_number: Some(p.chapter_number),
                published_at: p.published_at,
                is_short: book.map(|b| b.is_short).unwrap_or(false),
                is_songbook: book.map(|b| b.is_songbook).unwrap_or(false),
            },
        );
    }

    if meta.is_empty() {
        return Ok(Json(YoutubeVideoList {
            items: Vec::new(),
            total_views: 0,
            total_likes: 0,
            total_comments: 0,
            total_cost_usd: 0.0,
        }));
    }

    // Count videos per audiobook in the visible result so we can split
    // the audiobook's cost evenly across its rows. A single-mode video
    // is one row → full cost; a 10-chapter playlist is ten rows → 1/10
    // each. Summing the column then equals the audiobook total.
    let mut book_video_count: HashMap<String, u32> = HashMap::new();
    for m in meta.values() {
        *book_video_count.entry(m.audiobook_id.clone()).or_insert(0) += 1;
    }

    // YouTube's videos.list caps at 50 ids per call; chunk and
    // concatenate. The list is bounded by how many videos this user
    // has published, so a handful of round-trips at most.
    let ids: Vec<String> = meta.keys().cloned().collect();
    let mut stats: HashMap<String, yt_analytics::VideoStats> = HashMap::new();
    for chunk in ids.chunks(50) {
        let part = match yt_analytics::fetch_video_stats(&token, chunk).await {
            Ok(v) => v,
            Err(Error::Unauthorized) => {
                let _ = drop_account(&state, &user.id).await;
                return Err(Error::Unauthorized.into());
            }
            Err(e) => return Err(e.into()),
        };
        for s in part {
            stats.insert(s.video_id.clone(), s);
        }
    }

    let mut total_views = 0u64;
    let mut total_likes = 0u64;
    let mut total_comments = 0u64;
    let mut total_cost_usd = 0.0f64;
    let mut items: Vec<YoutubeVideoRow> = meta
        .into_iter()
        .map(|(vid, m)| {
            let s = stats.get(&vid);
            let views = s.map(|s| s.view_count).unwrap_or(0);
            let likes = s.map(|s| s.like_count).unwrap_or(0);
            let comments = s.map(|s| s.comment_count).unwrap_or(0);
            total_views += views;
            total_likes += likes;
            total_comments += comments;
            let book_total = book_cost.get(&m.audiobook_id).copied().unwrap_or(0.0);
            let share_denom = book_video_count
                .get(&m.audiobook_id)
                .copied()
                .unwrap_or(1)
                .max(1) as f64;
            let row_cost = book_total / share_denom;
            total_cost_usd += row_cost;
            YoutubeVideoRow {
                video_id: vid,
                audiobook_id: m.audiobook_id,
                audiobook_title: m.audiobook_title,
                chapter_number: m.chapter_number,
                published_at: m.published_at,
                view_count: views,
                like_count: likes,
                comment_count: comments,
                is_short: m.is_short,
                is_songbook: m.is_songbook,
                cost_usd: row_cost,
            }
        })
        .collect();
    // Most-watched on top — that's almost always what the dashboard
    // viewer is going to want.
    items.sort_by(|a, b| b.view_count.cmp(&a.view_count));

    Ok(Json(YoutubeVideoList {
        items,
        total_views,
        total_likes,
        total_comments,
        total_cost_usd,
    }))
}

// ---------------- /analytics/youtube/reports ------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeReportPoint {
    pub date: String,
    pub views: u64,
    pub likes: u64,
    pub comments: u64,
    /// `estimatedMinutesWatched` summed across the bucket. Useful as
    /// a watch-time proxy alongside raw views.
    pub estimated_minutes_watched: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct YoutubeReport {
    pub bucket: String,
    pub range_days: u32,
    pub points: Vec<YoutubeReportPoint>,
}

#[utoipa::path(
    get,
    path = "/analytics/youtube/reports",
    tag = "analytics",
    params(
        ("bucket" = Option<String>, Query, description = "day | week | month (default: day)"),
        ("range_days" = Option<u32>, Query, description = "Lookback window in days; auto-extends for week/month if absent")
    ),
    responses(
        (status = 200, description = "Time-series channel report via the YouTube Analytics API", body = YoutubeReport),
        (status = 401, description = "Unauthenticated, or YouTube grant was revoked at Google"),
        (status = 409, description = "User has not connected a YouTube channel")
    ),
    security(("bearer" = []))
)]
pub async fn youtube_reports(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Query(q): Query<AnalyticsQuery>,
) -> ApiResult<Json<YoutubeReport>> {
    let (bucket, since) = q.parsed()?;
    let range_days = (Utc::now() - since).num_days().max(1) as u32;
    let token = require_yt_token(&state, &user.id).await?;
    let start = since.date_naive();
    let end = Utc::now().date_naive();

    let rows = match yt_analytics::fetch_analytics_report(&token, start, end).await {
        Ok(r) => r,
        Err(Error::Unauthorized) => {
            let _ = drop_account(&state, &user.id).await;
            return Err(Error::Unauthorized.into());
        }
        Err(e) => return Err(e.into()),
    };

    // Collapse the daily rows into the requested bucket. For Day this
    // is a 1:1 pass; week/month sums each metric.
    let mut buckets: BTreeMap<String, YoutubeReportPoint> = BTreeMap::new();
    for r in rows {
        // Same noon-UTC anchor trick as `fill_empty_points`; total
        // over any valid `NaiveDate`.
        let dt = match r.date.and_hms_opt(12, 0, 0) {
            Some(t) => t.and_utc(),
            None => continue,
        };
        let key = bucket.key(dt);
        let entry = buckets.entry(key.clone()).or_insert_with(|| YoutubeReportPoint {
            date: key.clone(),
            views: 0,
            likes: 0,
            comments: 0,
            estimated_minutes_watched: 0,
        });
        entry.views += r.views;
        entry.likes += r.likes;
        entry.comments += r.comments;
        entry.estimated_minutes_watched += r.estimated_minutes_watched;
    }

    Ok(Json(YoutubeReport {
        bucket: match bucket {
            Bucket::Day => "day",
            Bucket::Week => "week",
            Bucket::Month => "month",
        }
        .to_string(),
        range_days,
        points: buckets.into_values().collect(),
    }))
}

// ---------------- shared YouTube preflight -------------------------------

/// Resolve an access token for this user, or surface a tidy 409 when
/// they haven't connected a channel yet. Same shape as the existing
/// publish-flow preflight so the frontend can reuse its "connect
/// YouTube first" surface.
async fn require_yt_token(state: &AppState, user: &UserId) -> Result<String> {
    match access_token(state, user).await? {
        Some(t) => Ok(t),
        None => Err(Error::Conflict("connect a YouTube channel first".into())),
    }
}
