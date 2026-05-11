//! YouTube read-side helpers powering the analytics dashboard.
//!
//! Three calls, kept narrow on purpose:
//!
//!   * [`fetch_channel_stats`] — `channels.list?mine=true&part=statistics,snippet`
//!     returns the subscriber / lifetime-view / video count tile.
//!   * [`fetch_video_stats`]   — `videos.list?id=…&part=statistics` chunks of
//!     ≤ 50 video ids and returns per-video view/like/comment counts.
//!   * [`fetch_analytics_report`] — YouTube Analytics API
//!     `reports?ids=channel==MINE&dimensions=day&metrics=…` for the
//!     watch-time + engagement time series.
//!
//! Each helper takes an already-refreshed access token (see
//! [`crate::youtube::account::access_token`]). A `401`/`403` from any
//! call surfaces as `Error::Unauthorized` so the calling handler can
//! drop the local account row and prompt the user to reconnect with
//! the broader (read-side) scopes.

use std::time::Duration;

use chrono::NaiveDate;
use listenai_core::{Error, Result};
use reqwest::Client;
use serde::Deserialize;

const CHANNELS_URL: &str = "https://www.googleapis.com/youtube/v3/channels";
const VIDEOS_URL: &str = "https://www.googleapis.com/youtube/v3/videos";
const ANALYTICS_URL: &str = "https://youtubeanalytics.googleapis.com/v2/reports";

/// One-shot HTTP client. Cheap to construct; we don't bother caching
/// across calls because reqwest already pools connections internally
/// and these are interactive (per-request) anyway.
fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("listenai-api/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Other(anyhow::anyhow!("yt http client: {e}")))
}

// ---------------- channel stats ------------------------------------------

#[derive(Debug)]
pub struct ChannelStats {
    pub channel_id: String,
    pub channel_title: String,
    pub subscriber_count: u64,
    pub view_count: u64,
    pub video_count: u64,
}

#[derive(Debug, Deserialize)]
struct ChannelsResp {
    #[serde(default)]
    items: Vec<ChannelsItem>,
}
#[derive(Debug, Deserialize)]
struct ChannelsItem {
    id: String,
    snippet: ChannelSnippet,
    #[serde(default)]
    statistics: Option<ChannelStatistics>,
}
#[derive(Debug, Deserialize)]
struct ChannelSnippet {
    title: String,
}
/// YouTube returns these counts as JSON strings, not numbers — match
/// the wire type and parse in Rust to avoid round-tripping through
/// `serde_json::Value`. Field names on the wire are camelCase
/// (`viewCount`, `subscriberCount`, `videoCount`); without the
/// rename serde silently zeros every field and the dashboard reads
/// blanks across the board.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelStatistics {
    #[serde(default)]
    view_count: Option<String>,
    #[serde(default)]
    subscriber_count: Option<String>,
    #[serde(default)]
    video_count: Option<String>,
}

pub async fn fetch_channel_stats(access_token: &str) -> Result<ChannelStats> {
    let http = http_client()?;
    let resp = http
        .get(CHANNELS_URL)
        .query(&[("part", "snippet,statistics"), ("mine", "true")])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt channel stats: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("yt channel stats read: {e}")))?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(Error::Unauthorized);
    }
    if !status.is_success() {
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt channel stats {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    let body: ChannelsResp = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("yt channel stats json: {e}")))?;
    let item = body.items.into_iter().next().ok_or_else(|| {
        Error::Upstream("yt channel stats: no channel for this account".into())
    })?;
    let stats = item.statistics.unwrap_or(ChannelStatistics {
        view_count: None,
        subscriber_count: None,
        video_count: None,
    });
    Ok(ChannelStats {
        channel_id: item.id,
        channel_title: item.snippet.title,
        subscriber_count: parse_u64(stats.subscriber_count.as_deref()),
        view_count: parse_u64(stats.view_count.as_deref()),
        video_count: parse_u64(stats.video_count.as_deref()),
    })
}

// ---------------- video stats --------------------------------------------

#[derive(Debug)]
pub struct VideoStats {
    pub video_id: String,
    pub view_count: u64,
    pub like_count: u64,
    pub comment_count: u64,
}

#[derive(Debug, Deserialize)]
struct VideosResp {
    #[serde(default)]
    items: Vec<VideosItem>,
}
#[derive(Debug, Deserialize)]
struct VideosItem {
    id: String,
    #[serde(default)]
    statistics: Option<VideoStatistics>,
}
/// Wire field names are camelCase (`viewCount`, `likeCount`,
/// `commentCount`). `rename_all` keeps the Rust names readable.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoStatistics {
    #[serde(default)]
    view_count: Option<String>,
    /// Channels that have hidden their like count return no field at
    /// all; we surface `0` rather than fail the whole request.
    #[serde(default)]
    like_count: Option<String>,
    #[serde(default)]
    comment_count: Option<String>,
}

/// Fetch statistics for up to 50 video ids in a single call. Callers
/// chunk longer lists themselves — kept simple here so the
/// concurrency story is the caller's choice.
pub async fn fetch_video_stats(access_token: &str, ids: &[String]) -> Result<Vec<VideoStats>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    if ids.len() > 50 {
        return Err(Error::Validation(format!(
            "fetch_video_stats accepts at most 50 ids per call, got {}",
            ids.len()
        )));
    }
    let http = http_client()?;
    let id_param = ids.join(",");
    let resp = http
        .get(VIDEOS_URL)
        .query(&[("part", "statistics"), ("id", id_param.as_str())])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt video stats: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("yt video stats read: {e}")))?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(Error::Unauthorized);
    }
    if !status.is_success() {
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt video stats {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    let body: VideosResp = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("yt video stats json: {e}")))?;
    Ok(body
        .items
        .into_iter()
        .map(|i| {
            let s = i.statistics.unwrap_or(VideoStatistics {
                view_count: None,
                like_count: None,
                comment_count: None,
            });
            VideoStats {
                video_id: i.id,
                view_count: parse_u64(s.view_count.as_deref()),
                like_count: parse_u64(s.like_count.as_deref()),
                comment_count: parse_u64(s.comment_count.as_deref()),
            }
        })
        .collect())
}

// ---------------- analytics report ---------------------------------------

#[derive(Debug)]
pub struct DailyReportRow {
    pub date: NaiveDate,
    pub views: u64,
    pub likes: u64,
    pub comments: u64,
    pub estimated_minutes_watched: u64,
}

/// The Analytics API returns a columnar payload: a `columnHeaders`
/// array describing the order, plus a `rows` array of mixed-type
/// values. We slice columns by name so a future re-ordering on
/// Google's side doesn't silently swap views for likes. The wire
/// field is `columnHeaders` — `rename_all = "camelCase"` keeps the
/// Rust name readable.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsResp {
    #[serde(default)]
    rows: Vec<Vec<serde_json::Value>>,
    #[serde(default)]
    column_headers: Vec<AnalyticsColumn>,
}
#[derive(Debug, Deserialize)]
struct AnalyticsColumn {
    name: String,
}

/// Fetch a day-bucketed channel-wide report. `views`, `likes`,
/// `comments`, and `estimatedMinutesWatched` are returned per day in
/// `[start_date, end_date]` inclusive.
pub async fn fetch_analytics_report(
    access_token: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<Vec<DailyReportRow>> {
    let http = http_client()?;
    let start = start_date.format("%Y-%m-%d").to_string();
    let end = end_date.format("%Y-%m-%d").to_string();
    let resp = http
        .get(ANALYTICS_URL)
        .query(&[
            ("ids", "channel==MINE"),
            ("startDate", start.as_str()),
            ("endDate", end.as_str()),
            ("metrics", "views,likes,comments,estimatedMinutesWatched"),
            ("dimensions", "day"),
            ("sort", "day"),
        ])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("yt analytics: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("yt analytics read: {e}")))?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(Error::Unauthorized);
    }
    if !status.is_success() {
        let preview = String::from_utf8_lossy(&bytes);
        return Err(Error::Upstream(format!(
            "yt analytics {status}: {}",
            preview.chars().take(400).collect::<String>()
        )));
    }
    let body: AnalyticsResp = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Upstream(format!("yt analytics json: {e}")))?;

    let col_index = |name: &str| {
        body.column_headers
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    };
    let day_idx = col_index("day").ok_or_else(|| {
        Error::Upstream("yt analytics: response missing `day` column".into())
    })?;
    // Missing metric columns degrade to zero rather than fail the
    // whole call — Analytics very occasionally omits a metric for a
    // brand-new channel with no engagement yet.
    let views_idx = col_index("views");
    let likes_idx = col_index("likes");
    let comments_idx = col_index("comments");
    let watch_idx = col_index("estimatedMinutesWatched");

    let mut out = Vec::with_capacity(body.rows.len());
    for row in body.rows {
        let day_str = row
            .get(day_idx)
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let Ok(date) = NaiveDate::parse_from_str(day_str, "%Y-%m-%d") else {
            continue;
        };
        let pick = |idx: Option<usize>| -> u64 {
            idx.and_then(|i| row.get(i))
                .map(|v| {
                    // Metrics arrive as floats (e.g. `12.0`); we
                    // truncate to integer for count metrics. Watch
                    // time is also a count of minutes, so the same
                    // treatment is fine.
                    v.as_f64()
                        .map(|f| f.max(0.0) as u64)
                        .or_else(|| v.as_u64())
                        .unwrap_or(0)
                })
                .unwrap_or(0)
        };
        out.push(DailyReportRow {
            date,
            views: pick(views_idx),
            likes: pick(likes_idx),
            comments: pick(comments_idx),
            estimated_minutes_watched: pick(watch_idx),
        });
    }
    Ok(out)
}

fn parse_u64(s: Option<&str>) -> u64 {
    s.and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: YouTube's statistics fields are camelCase
    /// (`viewCount`, not `view_count`). Without the rename serde
    /// silently dropped every count to `None` and the dashboard
    /// rendered zeros across the board.
    #[test]
    fn parses_camelcase_channel_statistics() {
        let json = serde_json::json!({
            "items": [{
                "id": "UC123",
                "snippet": { "title": "My Channel" },
                "statistics": {
                    "viewCount": "12345",
                    "subscriberCount": "678",
                    "videoCount": "9"
                }
            }]
        });
        let resp: ChannelsResp = serde_json::from_value(json).unwrap();
        let stats = resp.items[0].statistics.as_ref().unwrap();
        assert_eq!(stats.view_count.as_deref(), Some("12345"));
        assert_eq!(stats.subscriber_count.as_deref(), Some("678"));
        assert_eq!(stats.video_count.as_deref(), Some("9"));
    }

    #[test]
    fn parses_camelcase_video_statistics() {
        let json = serde_json::json!({
            "items": [{
                "id": "vid1",
                "statistics": {
                    "viewCount": "100",
                    "likeCount": "10",
                    "commentCount": "3"
                }
            }]
        });
        let resp: VideosResp = serde_json::from_value(json).unwrap();
        let stats = resp.items[0].statistics.as_ref().unwrap();
        assert_eq!(parse_u64(stats.view_count.as_deref()), 100);
        assert_eq!(parse_u64(stats.like_count.as_deref()), 10);
        assert_eq!(parse_u64(stats.comment_count.as_deref()), 3);
    }

    #[test]
    fn parses_camelcase_analytics_response() {
        let json = serde_json::json!({
            "columnHeaders": [
                { "name": "day" },
                { "name": "views" },
                { "name": "likes" },
                { "name": "comments" },
                { "name": "estimatedMinutesWatched" }
            ],
            "rows": [
                ["2026-05-01", 12.0, 1.0, 0.0, 5.0],
                ["2026-05-02", 7.0, 0.0, 0.0, 3.0]
            ]
        });
        let resp: AnalyticsResp = serde_json::from_value(json).unwrap();
        assert_eq!(resp.column_headers.len(), 5);
        assert_eq!(resp.rows.len(), 2);
        assert_eq!(resp.column_headers[0].name, "day");
        assert_eq!(resp.column_headers[4].name, "estimatedMinutesWatched");
    }
}
