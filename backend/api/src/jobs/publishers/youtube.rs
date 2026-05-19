//! `JobKind::PublishYoutube` handler.
//!
//! Two pipelines, picked from the publication row's `mode`:
//!
//!   * `mode = "single"` (the default):
//!       1. Resolve audiobook + chapters in the requested language; bail if
//!          any isn't `audio_ready` (defence in depth — the HTTP layer also
//!          checks).
//!       2. Refresh the user's OAuth access token. `invalid_grant` here
//!          means the user revoked us at Google; we delete the local row so
//!          the UI shows the "reconnect" prompt instead of looping forever.
//!       3. Locate the cover image on disk (cover.png/jpg/webp).
//!       4. Mux cover + concatenated chapter WAVs into a single MP4 with
//!          ffmpeg.
//!       5. Open a resumable upload session against YouTube and stream the
//!          bytes.
//!       6. Persist `video_id`, `video_url`, `published_at` on the
//!          publication row.
//!
//!   * `mode = "playlist"`:
//!       1. Same loading + auth as above.
//!       2. Create a YouTube playlist if the publication doesn't already
//!          have one. Re-running a partially-failed job reuses the existing
//!          playlist + skips chapters whose video row already has a
//!          `video_id`, so retries are idempotent.
//!       3. For each remaining chapter: encode chapter art (or the cover
//!          as fallback) + that chapter's WAV into an MP4, upload it, then
//!          append the resulting video to the playlist.
//!       4. Persist per-chapter rows in `youtube_publication_video` plus
//!          the playlist + final timestamp on the parent publication.

use std::path::{Path, PathBuf};

use std::time::Duration;

use async_trait::async_trait;
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use listenai_jobs::{
    handler::{JobContext, JobOutcome},
    repo::JobRow,
    JobHandler,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::state::AppState;
use crate::youtube::{account, playlist, subtitles, upload};

pub struct PublishYoutubeHandler(pub AppState);

#[async_trait]
impl JobHandler for PublishYoutubeHandler {
    async fn run(&self, ctx: &JobContext, job: JobRow) -> Result<JobOutcome> {
        let state = &self.0;
        let user_id = job
            .user_id
            .clone()
            .ok_or_else(|| Error::Database("publish_youtube: missing user".into()))?;
        let audiobook_id = job
            .audiobook_id
            .clone()
            .ok_or_else(|| Error::Database("publish_youtube: missing audiobook".into()))?;
        let language = job
            .language
            .clone()
            .ok_or_else(|| Error::Database("publish_youtube: missing language".into()))?;
        let user = UserId(user_id.clone());

        // Look the publication row up by (audiobook, language) — the unique
        // index. We deliberately don't trust the job payload for these
        // values; surrealdb's `option<object>` round-trip occasionally drops
        // string fields, and the publication row is the source of truth.
        let pub_row = match find_publication(state, &audiobook_id, &language).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return Ok(JobOutcome::Fatal(format!(
                    "no publication row for audiobook={audiobook_id} language={language}"
                )))
            }
            Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
        };
        let publication_id = pub_row.id;
        let privacy = pub_row.privacy_status;
        let mode = pub_row.mode;
        let existing_playlist_id = pub_row.playlist_id;
        let review = pub_row.review;
        let overlay_override = pub_row.like_subscribe_overlay;
        // Shorts always upload as a single vertical clip. Even if the
        // publication row was somehow flagged as `playlist`, override
        // that here so the encoder + uploader take the single-video
        // branch.

        // The description override is the only thing we still pull from the
        // payload because it doesn't live on the publication row.
        let description_override = job
            .payload
            .as_ref()
            .and_then(|p| p.get("description"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        // animate=true → use the per-chapter `ch-N.video.mp4` companion
        // videos as the visual track instead of the cover loop. The
        // HTTP handler already verifies they exist on disk before
        // enqueueing; we re-check inside the encoder to handle a stale
        // job whose chapter MP4s have been GC'd between enqueue and
        // pick-up.
        let animate = job
            .payload
            .as_ref()
            .and_then(|p| p.get("animate"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let hyperframes = job
            .payload
            .as_ref()
            .and_then(|p| p.get("hyperframes"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // None / 0 / 1 → renderer auto-scales by narration duration
        // (~1 step / 15 s, capped at 120). Anything else clamps in.
        let hyperframes_steps: Option<usize> = job
            .payload
            .as_ref()
            .and_then(|p| p.get("hyperframes_steps"))
            .and_then(|v| v.as_u64())
            .and_then(|n| {
                if n >= 2 {
                    Some(n.min(120) as usize)
                } else {
                    None
                }
            });

        tracing::debug!(
            job_id = %job.id,
            audiobook = %audiobook_id,
            language = %language,
            publication_id = %publication_id,
            privacy = %privacy,
            mode = %mode,
            review,
            "publish_youtube: starting"
        );

        // ---- 1. Load metadata. -------------------------------------------
        ctx.progress(&job, "loading", 0.0).await;
        let book = match load_audiobook(state, &audiobook_id).await {
            Ok(b) => b,
            Err(e) => return Ok(fail(state, &publication_id, e).await),
        };
        let chapters = match load_chapters(state, &audiobook_id, &language).await {
            Ok(c) => c,
            Err(e) => return Ok(fail(state, &publication_id, e).await),
        };
        // Per-language description footer (admin → YouTube settings).
        // Load once here so single/playlist/per-chapter builders below all
        // see the same value and a transient DB issue doesn't get retried
        // mid-publish.
        let footer_raw = load_description_footer(state, &language).await;
        // Optional "Models used" credits block. Off by default, opt-in via
        // /admin/youtube-publish-settings. We splice it into the same
        // string we hand downstream as `footer` so every description
        // builder picks it up without new parameters; the order in the
        // final description is: body → credits → admin footer.
        let publish_settings = crate::handlers::admin::load_youtube_publish_settings(state)
            .await
            .ok();
        let include_credits = publish_settings
            .as_ref()
            .map(|s| s.include_credits)
            .unwrap_or(false);
        // Per-publication override wins; otherwise fall back to the
        // singleton's default. `overlay_override` is `Some(_)` only when
        // the publish form explicitly set "Yes" or "No" — leaving the
        // tri-state on "Default" stores `NONE` and we inherit here.
        let like_subscribe_overlay = overlay_override.unwrap_or(
            publish_settings
                .as_ref()
                .map(|s| s.like_subscribe_overlay)
                .unwrap_or(false),
        );
        let credits = if include_credits {
            load_credits_block(state, &audiobook_id).await
        } else {
            None
        };
        let footer = combine_credits_and_footer(credits.as_deref(), footer_raw.as_deref());
        if chapters.is_empty() {
            return Ok(fail(
                state,
                &publication_id,
                Error::Conflict("no chapters in this language".into()),
            )
            .await);
        }
        if chapters.iter().any(|c| c.status != "audio_ready") {
            return Ok(fail(
                state,
                &publication_id,
                Error::Conflict("not every chapter is audio_ready".into()),
            )
            .await);
        }

        // ---- 2. Refresh OAuth access token (skipped in review mode). ----
        // Review mode never touches Google's APIs, so we don't need a token
        // and don't want a transient OAuth issue to block a local preview.
        ctx.progress(&job, "auth", 0.05).await;
        let access_token = if review {
            String::new()
        } else {
            match resolve_access_token(state, &user).await {
                Ok(t) => t,
                Err(e) => {
                    // Unauthorized = the user revoked us at Google. Clean
                    // up the local row so the UI prompts to reconnect;
                    // this is fatal.
                    if matches!(e, Error::Unauthorized) {
                        drop_account(state, &user).await.ok();
                        return Ok(fail(state, &publication_id, Error::Unauthorized).await);
                    }
                    return Ok(JobOutcome::Retry(e.to_string()));
                }
            }
        };

        // ---- 3. Locate cover image. --------------------------------------
        let cover_path = match find_cover(state, &audiobook_id) {
            Some(p) => p,
            None => {
                return Ok(fail(
                    state,
                    &publication_id,
                    Error::Conflict("audiobook has no cover image".into()),
                )
                .await)
            }
        };

        // The audiobook's podcast (if assigned + synced) provides an
        // umbrella playlist that takes precedence over per-publication
        // playlists in playlist mode and gets the single-mode video
        // appended after upload.
        let podcast_playlist_id = if review {
            None
        } else {
            load_podcast_playlist(state, &audiobook_id)
                .await
                .ok()
                .flatten()
        };

        let effective_mode = if book.is_short.unwrap_or(false) {
            "single"
        } else {
            mode.as_str()
        };
        // Shorts use a 9:16 cover composite which the 16:9 per-chapter
        // animation videos can't contribute to. Refuse rather than
        // produce a letterboxed frame the platform might down-rank.
        if animate && book.is_short.unwrap_or(false) {
            return Ok(fail(
                state,
                &publication_id,
                Error::Conflict(
                    "animate=true is incompatible with Shorts (vertical 9:16 not supported by the 16:9 chapter renders)".into(),
                ),
            )
            .await);
        }
        // Hyperframes is its own visual track (built from book content
        // by the render server), so combining it with the per-chapter
        // animations would just throw one of the tracks away. Reject
        // the combination loudly — the HTTP layer also catches it but
        // a stale job from before that check could still arrive here.
        if animate && hyperframes {
            return Ok(fail(
                state,
                &publication_id,
                Error::Conflict("animate and hyperframes are mutually exclusive — pick one".into()),
            )
            .await);
        }
        match effective_mode {
            "playlist" => {
                // Prefer the podcast's playlist over a per-publication one
                // when both exist — the podcast is the more durable
                // grouping. Falls back to the publication's `playlist_id`
                // (resume support) and finally to creating a fresh one.
                let from_podcast = podcast_playlist_id.is_some();
                let playlist_for_run = podcast_playlist_id
                    .as_deref()
                    .or(existing_playlist_id.as_deref());
                run_playlist(
                    state,
                    ctx,
                    &job,
                    &user,
                    &audiobook_id,
                    &language,
                    &publication_id,
                    &privacy,
                    &access_token,
                    &book,
                    &chapters,
                    &cover_path,
                    playlist_for_run,
                    from_podcast,
                    description_override.as_deref(),
                    footer.as_deref(),
                    review,
                    animate,
                    hyperframes,
                    hyperframes_steps,
                    like_subscribe_overlay,
                )
                .await
            }
            // "single" or anything unexpected → safe default.
            _ => {
                run_single(
                    state,
                    ctx,
                    &job,
                    &user,
                    &audiobook_id,
                    &language,
                    &publication_id,
                    &privacy,
                    &access_token,
                    &book,
                    &chapters,
                    &cover_path,
                    podcast_playlist_id.as_deref(),
                    description_override.as_deref(),
                    footer.as_deref(),
                    review,
                    animate,
                    hyperframes,
                    hyperframes_steps,
                    like_subscribe_overlay,
                )
                .await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Mode: single video
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_single(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    user: &UserId,
    audiobook_id: &str,
    language: &str,
    publication_id: &str,
    privacy: &str,
    access_token: &str,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    cover_path: &Path,
    podcast_playlist_id: Option<&str>,
    description_override: Option<&str>,
    footer: Option<&str>,
    review: bool,
    animate: bool,
    hyperframes: bool,
    hyperframes_steps: Option<usize>,
    like_subscribe_overlay: bool,
) -> Result<JobOutcome> {
    // ---- ffmpeg encode. ----------------------------------------------------
    //
    // Two paths:
    //   * `animate=false` (the original): build one image-loop segment per
    //     chapter (chapter art when present, cover otherwise) and splice
    //     them together with their per-chapter WAVs. Chapters that don't
    //     have art all reuse the cover composite, so this stays as cheap
    //     as the old single-image path on books with no art.
    //   * `animate=true` (Phase D): the per-chapter `ch-N.video.mp4`
    //     companions already have visuals + audio baked in. Just concat
    //     them with `-c copy`, no re-encode.
    ctx.progress(job, "encoding", 0.1).await;
    let mp4_path = state
        .config()
        .storage_path
        .join(audiobook_id)
        .join(language)
        .join("youtube.mp4");

    let storage = &state.config().storage_path;
    let is_short = book.is_short.unwrap_or(false);

    if hyperframes {
        // Shorts (≤ 90 s) fit in a single round-trip to the
        // Hyperframes service. Long-form would otherwise blow past the
        // service's per-render budget, so we split the book into
        // per-chapter compositions and ffmpeg-concat them at the end.
        // The user's requested `hyperframes_steps` is treated as a
        // book-level total and allocated across chapters by duration.
        if is_short {
            let total_ms: u64 = chapters
                .iter()
                .map(|c| c.duration_ms.unwrap_or(0).max(0) as u64)
                .sum();
            if let Err(e) = render_hyperframes_video(
                state,
                ctx,
                job,
                book,
                chapters,
                audiobook_id,
                language,
                cover_path,
                total_ms,
                hyperframes_steps,
                true,
                "all",
                &mp4_path,
            )
            .await
            {
                return Ok(fail(state, publication_id, e).await);
            }
        } else if let Err(e) = render_hyperframes_long_chunked(
            state,
            ctx,
            job,
            book,
            chapters,
            audiobook_id,
            language,
            cover_path,
            hyperframes_steps,
            &mp4_path,
        )
        .await
        {
            return Ok(fail(state, publication_id, e).await);
        }
    } else if animate {
        let chapter_videos: Vec<PathBuf> = chapters
            .iter()
            .map(|c| {
                storage
                    .join(audiobook_id)
                    .join(language)
                    .join(format!("ch-{}.video.mp4", c.number))
            })
            .collect();
        for p in &chapter_videos {
            if !p.exists() {
                return Ok(fail(
                    state,
                    publication_id,
                    Error::Conflict(format!(
                        "animation missing on disk: {} (re-run /animate)",
                        p.display()
                    )),
                )
                .await);
            }
        }
        if let Err(e) = concat_animated_chapters(state, &chapter_videos, &mp4_path).await {
            return Ok(fail(state, publication_id, e).await);
        }
    } else {
        // Image segments: paragraph-weighted slideshow per chapter, with
        // chapter art lead-ins. Falls back to one segment per chapter
        // when no paragraph illustrations exist. Shorts use the same
        // build path — the encoder composites each tile onto the 9:16
        // blurred-backdrop layout so square paragraph art displays as a
        // 1080×1080 inset, identical treatment to the cover. Books
        // without paragraph art (or Shorts with `images_per_paragraph =
        // 0`) get a single full-duration cover/chapter-art segment via
        // the same fallback as before.
        // Songbook snippets — interleave one snippet WAV between
        // chapter i and chapter i+1 for each available
        // `snippet-<i>.wav` on disk. Missing files are tolerated:
        // the snippet job is best-effort, so a partial set is normal.
        // The visual track during a snippet just shows the cover.
        let snippet_dir = state
            .config()
            .storage_path
            .join(audiobook_id)
            .join("snippets");
        let snippet_count = book.snippet_count.unwrap_or(0).clamp(0, 12) as u32;

        let mut image_segments: Vec<ImageSegment> = Vec::new();
        let mut wavs: Vec<PathBuf> = Vec::new();
        let mut spliced_snippet_ms: u64 = 0;
        for (idx, c) in chapters.iter().enumerate() {
            image_segments.extend(build_chapter_image_segments(
                c, cover_path, storage, is_short,
            ));
            wavs.push(
                state
                    .config()
                    .storage_path
                    .join(audiobook_id)
                    .join(language)
                    .join(format!("ch-{}.wav", c.number)),
            );
            let snippet_idx = (idx as u32) + 1;
            if snippet_idx <= snippet_count {
                let path = snippet_dir.join(format!("snippet-{snippet_idx}.wav"));
                if path.exists() {
                    match wav_duration_ms(&path) {
                        Ok(dur_ms) if dur_ms > 0 => {
                            wavs.push(path.clone());
                            image_segments.push(ImageSegment {
                                image_src: cover_path.to_path_buf(),
                                duration_ms: dur_ms,
                            });
                            spliced_snippet_ms += dur_ms;
                        }
                        Ok(_) => {
                            warn!(
                                snippet = ?path,
                                "publish: snippet has zero duration; skipping"
                            );
                        }
                        Err(e) => {
                            warn!(
                                snippet = ?path,
                                error = %e,
                                "publish: probe snippet wav failed; skipping"
                            );
                        }
                    }
                }
            }
        }

        // Map ffmpeg's [0..1] encode progress onto [0.10..0.30] of the
        // overall publish progress so the bar moves smoothly through the
        // longest phase. Sum chapter durations to give the parser something
        // to compute against.
        let total_ms: u64 = chapters
            .iter()
            .map(|c| c.duration_ms.unwrap_or(0).max(0) as u64)
            .sum::<u64>()
            + spliced_snippet_ms;

        let job_for_encode = job.clone();
        let ctx_for_encode = ctx.clone();
        let encode_result = encode_mp4_segmented(
            state,
            &image_segments,
            &wavs,
            &mp4_path,
            total_ms,
            is_short,
            like_subscribe_overlay,
            move |frac| {
                let job = job_for_encode.clone();
                let ctx = ctx_for_encode.clone();
                async move {
                    let overall = 0.10 + (frac * 0.20);
                    ctx.progress(&job, "encoding", overall.clamp(0.0, 0.30))
                        .await;
                }
            },
        )
        .await;
        if let Err(e) = encode_result {
            return Ok(fail(state, publication_id, e).await);
        }
    }
    ctx.progress(job, "encoded", 0.3).await;

    if review {
        if let Err(e) = mark_preview_ready(state, publication_id).await {
            warn!(error = %e, "publish_youtube: mark preview_ready failed");
        }
        info!(
            audiobook = %audiobook_id,
            language = %language,
            mp4 = ?mp4_path,
            "youtube preview ready (single, awaiting approval)"
        );
        ctx.progress(job, "preview_ready", 1.0).await;
        return Ok(JobOutcome::Done);
    }

    // ---- Resumable upload. -------------------------------------------------
    let metadata = build_book_metadata(
        book,
        chapters,
        language,
        privacy,
        description_override,
        footer,
    );
    let upload_result =
        match upload_one(ctx, job, access_token, &mp4_path, &metadata, 0.35, 0.99).await {
            Ok(r) => r,
            Err(Error::Unauthorized) => {
                drop_account(state, user).await.ok();
                return Ok(fail(state, publication_id, Error::Unauthorized).await);
            }
            Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
        };

    let video_url = format!("https://youtu.be/{}", upload_result.video_id);
    if let Err(e) = mark_published(state, publication_id, &upload_result.video_id, &video_url).await
    {
        warn!(error = %e, "publish_youtube: persist result failed");
    }

    // Drop the new video into the audiobook's podcast playlist when one
    // exists. Best-effort: a 4xx here is annoying but not worth rolling
    // the upload back — the user can re-add the video on YouTube directly
    // or re-trigger the publish, which is idempotent.
    if let Some(playlist_id) = podcast_playlist_id {
        match playlist::add_video(access_token, playlist_id, &upload_result.video_id, None).await {
            Ok(()) => {
                let playlist_url = format!("https://www.youtube.com/playlist?list={playlist_id}");
                if let Err(e) =
                    mark_playlist_created(state, publication_id, playlist_id, &playlist_url).await
                {
                    warn!(error = %e, "publish_youtube: persist podcast playlist failed");
                }
                // Now that the playlist has at least one episode, try to
                // flip it into an actual YouTube podcast. Best-effort:
                // YouTube can still reject this for other reasons (e.g.
                // channel not eligible), and the user can always re-try
                // via the manual sync button.
                try_designate_podcast(state, access_token, playlist_id, &book.title, language)
                    .await;
            }
            Err(Error::Unauthorized) => {
                drop_account(state, user).await.ok();
                // Don't fail the publish — the video is already up.
                warn!(
                    audiobook = %audiobook_id,
                    "publish_youtube: podcast playlist add failed (unauthorized)"
                );
            }
            Err(e) => {
                warn!(
                    error = %e,
                    audiobook = %audiobook_id,
                    playlist_id,
                    "publish_youtube: podcast playlist add failed"
                );
            }
        }
    }

    // Best-effort caption upload — failures here don't roll back a
    // successful video publish. Uploads one CC track per language that
    // has chapter text on this audiobook (the viewer picks via CC menu);
    // all tracks share the primary language's chapter durations because
    // that's the audio playing in the video.
    upload_book_captions(
        state,
        access_token,
        audiobook_id,
        &upload_result.video_id,
        chapters,
        language,
    )
    .await;

    info!(
        audiobook = %audiobook_id,
        language = %language,
        video_id = %upload_result.video_id,
        "youtube publish complete"
    );
    ctx.progress(job, "completed", 1.0).await;
    Ok(JobOutcome::Done)
}

// ---------------------------------------------------------------------------
// Mode: playlist (one video per chapter)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_playlist(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    user: &UserId,
    audiobook_id: &str,
    language: &str,
    publication_id: &str,
    privacy: &str,
    access_token: &str,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    cover_path: &Path,
    existing_playlist_id: Option<&str>,
    // `playlist_is_podcast`: true when the playlist we're publishing
    // into is the podcast's umbrella playlist; we'll try to flip its
    // `podcastStatus` to `enabled` after the first video lands.
    playlist_is_podcast: bool,
    description_override: Option<&str>,
    footer: Option<&str>,
    review: bool,
    animate: bool,
    hyperframes: bool,
    hyperframes_steps: Option<usize>,
    like_subscribe_overlay: bool,
) -> Result<JobOutcome> {
    // Review mode: just encode every chapter MP4 and stop. We deliberately
    // don't create the playlist on YouTube here — that's a side effect the
    // user explicitly approves at the next step.
    if review {
        return run_playlist_preview(
            state,
            ctx,
            job,
            audiobook_id,
            language,
            publication_id,
            book,
            chapters,
            cover_path,
            animate,
            hyperframes,
            hyperframes_steps,
            like_subscribe_overlay,
        )
        .await;
    }

    // ---- Ensure playlist exists. ------------------------------------------
    ctx.progress(job, "playlist", 0.06).await;
    let playlist = match existing_playlist_id {
        Some(id) if !id.trim().is_empty() => playlist::Playlist {
            id: id.to_string(),
            url: format!("https://www.youtube.com/playlist?list={id}"),
        },
        _ => {
            let title = trim_to(&sanitize_for_youtube(&book.title), 150);
            let description = trim_to(
                &sanitize_for_youtube(&render_playlist_description(
                    book,
                    description_override,
                    footer,
                    language,
                )),
                5000,
            );
            match playlist::create_playlist(
                access_token,
                &title,
                &description,
                privacy,
                Some(language),
                false,
            )
            .await
            {
                Ok(p) => {
                    if let Err(e) =
                        mark_playlist_created(state, publication_id, &p.id, &p.url).await
                    {
                        warn!(error = %e, "publish_youtube: persist playlist failed");
                    }
                    p
                }
                Err(Error::Unauthorized) => {
                    drop_account(state, user).await.ok();
                    return Ok(fail(state, publication_id, Error::Unauthorized).await);
                }
                Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
            }
        }
    };

    // ---- Resume support: which chapters already have a video? -------------
    let existing_videos = match load_publication_videos(state, publication_id).await {
        Ok(v) => v,
        Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
    };

    // Progress envelope: [0.10 .. 0.99] split evenly across remaining chapters.
    let total = chapters.len().max(1);
    let done_already = existing_videos
        .iter()
        .filter(|v| v.video_id.is_some())
        .count();
    info!(
        audiobook = %audiobook_id,
        language = %language,
        total,
        done_already,
        playlist_id = %playlist.id,
        "publish_youtube: playlist mode starting"
    );

    for (idx, ch) in chapters.iter().enumerate() {
        let span_start = 0.10 + (idx as f32 / total as f32) * 0.89;
        let span_end = 0.10 + ((idx + 1) as f32 / total as f32) * 0.89;

        let prior = existing_videos
            .iter()
            .find(|v| v.chapter_number == ch.number);
        if let Some(prior) = prior {
            if prior.video_id.is_some() {
                ctx.progress(
                    job,
                    &format!("ch{} skipped", ch.number),
                    span_end.clamp(0.0, 0.99),
                )
                .await;
                continue;
            }
        }

        let storage = &state.config().storage_path;

        // hyperframes=true: render a content-aware composition for
        // this chapter via the Hyperframes service, then mux with the
        // chapter's WAV. Same orientation as the book (Shorts get the
        // 9:16 short layout, otherwise 16:9 landscape via rotation +
        // transpose). Skips the slideshow path below.
        let mp4_path = if hyperframes {
            let path = state
                .config()
                .storage_path
                .join(audiobook_id)
                .join(language)
                .join(format!("youtube-ch-{}.mp4", ch.number));
            let chapter_total_ms = ch.duration_ms.unwrap_or(0).max(0) as u64;
            let suffix = format!("ch-{}", ch.number);
            // Per-chapter scope so each chapter's render server entry
            // is independent (we delete it on completion either way).
            if let Err(e) = render_hyperframes_video(
                state,
                ctx,
                job,
                book,
                std::slice::from_ref(ch),
                audiobook_id,
                language,
                cover_path,
                chapter_total_ms,
                hyperframes_steps,
                false, // playlist mode is gated to non-Shorts at dispatch.
                &suffix,
                &path,
            )
            .await
            {
                mark_chapter_video_error(state, publication_id, ch, &e.to_string()).await;
                return Ok(fail(state, publication_id, e).await);
            }
            // Encode-stage progress: jump to the encode-end milestone
            // (~30 % of the chapter span) — the renderer reports its
            // own progress sub-stages already.
            let span_30 = span_start + (span_end - span_start) * 0.30;
            ctx.progress(
                job,
                &format!("ch{} hyperframes ready", ch.number),
                span_30.clamp(0.0, 0.99),
            )
            .await;
            path
        } else if animate {
            let p = storage
                .join(audiobook_id)
                .join(language)
                .join(format!("ch-{}.video.mp4", ch.number));
            if !p.exists() {
                let err = Error::Conflict(format!(
                    "animation missing on disk: {} (re-run /animate)",
                    p.display()
                ));
                mark_chapter_video_error(state, publication_id, ch, &err.to_string()).await;
                return Ok(fail(state, publication_id, err).await);
            }
            // Encode-stage progress jumps straight to the encode-end
            // milestone since there's no encode happening.
            let span_30 = span_start + (span_end - span_start) * 0.30;
            ctx.progress(
                job,
                &format!("ch{} ready", ch.number),
                span_30.clamp(0.0, 0.99),
            )
            .await;
            p
        } else {
            // Build the per-chapter slideshow: paragraph-weighted segments
            // when the chapter has paragraph illustrations, otherwise a
            // single full-chapter segment with chapter art (or cover).
            // Playlist mode is gated to non-Short books at the dispatch
            // site, so the standard slideshow tuning applies.
            let image_segments = build_chapter_image_segments(ch, cover_path, storage, false);

            let chapter_wav = state
                .config()
                .storage_path
                .join(audiobook_id)
                .join(language)
                .join(format!("ch-{}.wav", ch.number));

            // Per-chapter MP4 in its own subdir so retries don't trip over a
            // half-written file from the previous attempt.
            let path = state
                .config()
                .storage_path
                .join(audiobook_id)
                .join(language)
                .join(format!("youtube-ch-{}.mp4", ch.number));

            // Map ffmpeg progress onto [span_start .. span_start + 30% of span].
            let encode_span_end = span_start + (span_end - span_start) * 0.30;
            let encode_total_ms = ch.duration_ms.unwrap_or(0).max(0) as u64;
            let job_for_encode = job.clone();
            let ctx_for_encode = ctx.clone();
            let stage_label = format!("ch{} encoding", ch.number);
            let encode_result = encode_mp4_segmented(
                state,
                &image_segments,
                std::slice::from_ref(&chapter_wav),
                &path,
                encode_total_ms,
                // Playlist mode is gated to non-Short books at the dispatch
                // site, so a horizontal encode is always correct here.
                false,
                like_subscribe_overlay,
                move |frac| {
                    let job = job_for_encode.clone();
                    let ctx = ctx_for_encode.clone();
                    let label = stage_label.clone();
                    async move {
                        let overall = span_start + frac * (encode_span_end - span_start);
                        ctx.progress(&job, &label, overall.clamp(0.0, 0.99)).await;
                    }
                },
            )
            .await;
            if let Err(e) = encode_result {
                // Persist the per-chapter error so the UI can show which one
                // broke without losing the chapters that already succeeded.
                mark_chapter_video_error(state, publication_id, ch, &e.to_string()).await;
                return Ok(fail(state, publication_id, e).await);
            }
            path
        };
        let encode_span_end = span_start + (span_end - span_start) * 0.30;

        // Pre-persist the chapter video row (without an id) so a half-
        // failed upload still leaves a marker the UI can show.
        if let Err(e) = upsert_chapter_video_pending(state, publication_id, ch).await {
            warn!(error = %e, "publish_youtube: pre-persist chapter video failed");
        }

        let metadata =
            build_chapter_metadata(book, ch, chapters.len() as u32, language, privacy, footer);
        let upload_result = match upload_one(
            ctx,
            job,
            access_token,
            &mp4_path,
            &metadata,
            encode_span_end,
            span_end - 0.005,
        )
        .await
        {
            Ok(r) => r,
            Err(Error::Unauthorized) => {
                drop_account(state, user).await.ok();
                mark_chapter_video_error(state, publication_id, ch, "unauthorized").await;
                return Ok(fail(state, publication_id, Error::Unauthorized).await);
            }
            Err(e) => {
                mark_chapter_video_error(state, publication_id, ch, &e.to_string()).await;
                return Ok(JobOutcome::Retry(e.to_string()));
            }
        };

        let video_url = format!("https://youtu.be/{}", upload_result.video_id);
        if let Err(e) = mark_chapter_video_published(
            state,
            publication_id,
            ch,
            &upload_result.video_id,
            &video_url,
        )
        .await
        {
            warn!(error = %e, "publish_youtube: persist chapter video failed");
        }

        // Best-effort custom thumbnail so each chapter tile in the
        // playlist shows its own art rather than YouTube's auto-pick
        // (which collapses to a near-identical frame across chapters).
        upload_chapter_thumbnail(state, access_token, &upload_result.video_id, ch, cover_path)
            .await;

        // Best-effort caption upload — failures here don't fail the
        // chapter publish, but they're logged so admins can re-attempt.
        upload_chapter_captions(access_token, &upload_result.video_id, ch, language).await;

        // Append to the playlist. A failure here is annoying but not fatal —
        // the video is uploaded; the user can add it manually if needed.
        // Bubble it as a retry so the worker re-runs and idempotently
        // re-tries the playlist add (videos with stored ids will be
        // skipped above).
        match playlist::add_video(
            access_token,
            &playlist.id,
            &upload_result.video_id,
            Some(idx as u32),
        )
        .await
        {
            Ok(()) => {
                // Only the podcast's umbrella playlist gets the podcast
                // designation flip; per-publication chapter playlists
                // stay as regular playlists. We try once per run after
                // the first video lands — the call is idempotent so
                // repeating it on later episodes is cheap.
                if playlist_is_podcast && idx == 0 {
                    try_designate_podcast(state, access_token, &playlist.id, &book.title, language)
                        .await;
                }
            }
            Err(Error::Unauthorized) => {
                drop_account(state, user).await.ok();
                return Ok(fail(state, publication_id, Error::Unauthorized).await);
            }
            Err(e) => return Ok(JobOutcome::Retry(e.to_string())),
        }

        // Best-effort cleanup: each chapter MP4 is regenerable.
        let _ = tokio::fs::remove_file(&mp4_path).await;
    }

    if let Err(e) = mark_playlist_complete(state, publication_id).await {
        warn!(error = %e, "publish_youtube: mark complete failed");
    }
    info!(
        audiobook = %audiobook_id,
        language = %language,
        playlist_id = %playlist.id,
        "youtube playlist publish complete"
    );
    ctx.progress(job, "completed", 1.0).await;
    Ok(JobOutcome::Done)
}

/// Review-mode playlist branch: encode each chapter MP4 and stop. The
/// per-chapter video rows are *not* persisted yet — those track YouTube
/// uploads, and there are none in review mode. The user previews the MP4s
/// via the streaming endpoint, then approves to enqueue the real upload
/// run.
#[allow(clippy::too_many_arguments)]
async fn run_playlist_preview(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    audiobook_id: &str,
    language: &str,
    publication_id: &str,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    cover_path: &Path,
    animate: bool,
    hyperframes: bool,
    hyperframes_steps: Option<usize>,
    like_subscribe_overlay: bool,
) -> Result<JobOutcome> {
    let total = chapters.len().max(1);
    for (idx, ch) in chapters.iter().enumerate() {
        let span_start = 0.10 + (idx as f32 / total as f32) * 0.89;
        let span_end = 0.10 + ((idx + 1) as f32 / total as f32) * 0.89;

        let storage = &state.config().storage_path;

        if hyperframes {
            let path = state
                .config()
                .storage_path
                .join(audiobook_id)
                .join(language)
                .join(format!("youtube-ch-{}.mp4", ch.number));
            let chapter_total_ms = ch.duration_ms.unwrap_or(0).max(0) as u64;
            let suffix = format!("ch-{}", ch.number);
            if let Err(e) = render_hyperframes_video(
                state,
                ctx,
                job,
                book,
                std::slice::from_ref(ch),
                audiobook_id,
                language,
                cover_path,
                chapter_total_ms,
                hyperframes_steps,
                false,
                &suffix,
                &path,
            )
            .await
            {
                return Ok(fail(state, publication_id, e).await);
            }
            ctx.progress(
                job,
                &format!("ch{} hyperframes ready", ch.number),
                span_end.clamp(0.0, 0.99),
            )
            .await;
            continue;
        }

        if animate {
            // Animation already exists on disk; nothing to do but
            // verify and bump progress. Preview UI streams it from the
            // existing path.
            let p = storage
                .join(audiobook_id)
                .join(language)
                .join(format!("ch-{}.video.mp4", ch.number));
            if !p.exists() {
                return Ok(fail(
                    state,
                    publication_id,
                    Error::Conflict(format!(
                        "animation missing on disk: {} (re-run /animate)",
                        p.display()
                    )),
                )
                .await);
            }
            ctx.progress(
                job,
                &format!("ch{} ready", ch.number),
                span_end.clamp(0.0, 0.99),
            )
            .await;
            continue;
        }

        // Playlist preview is gated to non-Short books (Shorts force
        // single mode), so the standard slideshow tuning applies.
        let image_segments = build_chapter_image_segments(ch, cover_path, storage, false);

        let chapter_wav = state
            .config()
            .storage_path
            .join(audiobook_id)
            .join(language)
            .join(format!("ch-{}.wav", ch.number));
        let mp4_path = state
            .config()
            .storage_path
            .join(audiobook_id)
            .join(language)
            .join(format!("youtube-ch-{}.mp4", ch.number));

        let encode_total_ms = ch.duration_ms.unwrap_or(0).max(0) as u64;
        let job_for_encode = job.clone();
        let ctx_for_encode = ctx.clone();
        let stage_label = format!("ch{} encoding", ch.number);
        let encode_result = encode_mp4_segmented(
            state,
            &image_segments,
            std::slice::from_ref(&chapter_wav),
            &mp4_path,
            encode_total_ms,
            // Playlist preview always renders horizontal — Shorts use
            // the single-video branch.
            false,
            like_subscribe_overlay,
            move |frac| {
                let job = job_for_encode.clone();
                let ctx = ctx_for_encode.clone();
                let label = stage_label.clone();
                async move {
                    let overall = span_start + frac * (span_end - span_start);
                    ctx.progress(&job, &label, overall.clamp(0.0, 0.99)).await;
                }
            },
        )
        .await;
        if let Err(e) = encode_result {
            return Ok(fail(state, publication_id, e).await);
        }
    }

    if let Err(e) = mark_preview_ready(state, publication_id).await {
        warn!(error = %e, "publish_youtube: mark preview_ready failed");
    }
    info!(
        audiobook = %audiobook_id,
        language = %language,
        chapters = chapters.len(),
        "youtube preview ready (playlist, awaiting approval)"
    );
    ctx.progress(job, "preview_ready", 1.0).await;
    Ok(JobOutcome::Done)
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

use crate::generation::song_snippets::wav_duration_ms;

/// One image slot in the slideshow video track: the image to display
/// and how long it stays on screen. Sum of every segment's duration_ms
/// must equal the sum of the wav file durations being muxed in.
struct ImageSegment {
    /// Absolute path to the source image (cover, chapter tile, or
    /// paragraph illustration).
    image_src: PathBuf,
    /// How long this image stays on screen.
    duration_ms: u64,
}

/// Minimum on-screen time per slideshow image. Below this an image
/// barely registers and ffmpeg has to do a lot of stitching for very
/// little screen time. We cap the number of slides per chapter to
/// keep each one ≥ this threshold.
const MIN_SEGMENT_MS: u64 = 2000;

/// Build the slideshow segments for one chapter. Mirrors the player's
/// `ChapterSlideshow` algorithm: a chapter-art tile lead-in (sized like
/// one average visual paragraph) followed by one segment per paragraph
/// tile, weighted by paragraph character count over the chapter total.
///
/// Falls back to a single full-chapter segment using `chapter_art_path`
/// (or `cover_path` when missing) for chapters with no paragraph images
/// — this is the legacy behaviour and what publications in translated
/// languages get (paragraph illustrations are anchored to the primary
/// chapter row).
///
/// `is_short` tunes the algorithm for ≤ 90 s vertical clips: the
/// chapter-art "establishing shot" lead-in is skipped (each slot is
/// too precious to spend on a near-duplicate of the cover) and the
/// per-slide minimum drops to 700 ms (TikTok-paced viewers tolerate
/// quick cuts, and the lower floor lets every generated tile fit even
/// in a 30 s clip).
fn build_chapter_image_segments(
    chapter: &DbChapter,
    cover_path: &Path,
    storage_path: &Path,
    is_short: bool,
) -> Vec<ImageSegment> {
    let chapter_duration_ms = chapter.duration_ms.unwrap_or(0).max(0) as u64;

    // Chapter art tile, with cover-art fallback.
    let chapter_art = chapter
        .chapter_art_path
        .as_deref()
        .map(|rel| storage_path.join(rel))
        .filter(|p| p.exists())
        .unwrap_or_else(|| cover_path.to_path_buf());

    // Visual paragraphs that actually have at least one persisted tile.
    // We resolve and `exists()`-check tile paths up front so a half-
    // failed image-gen doesn't feed ffmpeg a missing file. The
    // `scene_description` filter is intentionally permissive — older
    // tiles generated before that field landed are still kept as long
    // as the file exists on disk.
    let visual: Vec<(u64, Vec<PathBuf>)> = chapter
        .paragraphs
        .as_ref()
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    let tiles: Vec<PathBuf> = p
                        .image_paths
                        .iter()
                        .filter(|s| !s.trim().is_empty())
                        .map(|rel| storage_path.join(rel))
                        .filter(|p| p.exists())
                        .collect();
                    if tiles.is_empty() {
                        return None;
                    }
                    let chars = p.char_count.unwrap_or(0).max(1) as u64;
                    Some((chars, tiles))
                })
                .collect()
        })
        .unwrap_or_default();

    if visual.is_empty() || chapter_duration_ms == 0 {
        return vec![ImageSegment {
            image_src: chapter_art,
            duration_ms: chapter_duration_ms,
        }];
    }

    let mut slides: Vec<(PathBuf, u64)> = Vec::new();
    if !is_short {
        // Establishing-shot lead-in for full-length books — sized like
        // one average visual paragraph so the chapter art gets a clear
        // moment before the slideshow starts. Shorts skip this so every
        // slot can host a generated tile.
        let total_chars: u64 = visual.iter().map(|(c, _)| *c).sum();
        let avg_chars = (total_chars / visual.len() as u64).max(1);
        slides.push((chapter_art.clone(), avg_chars));
    }
    for (chars, tiles) in &visual {
        let per_tile = (chars / tiles.len() as u64).max(1);
        for tile_path in tiles {
            slides.push((tile_path.clone(), per_tile));
        }
    }

    // Cap so each segment gets at least the chosen minimum on screen.
    // Shorts use a smaller floor because the format expects quick cuts,
    // and the lower floor lets every generated tile fit in a 30–90 s
    // clip. Drop the tail rather than silently squeezing every slot
    // below the minimum — earlier slides are the most narratively
    // important.
    let min_segment_ms = if is_short { 700 } else { MIN_SEGMENT_MS };
    let max_slides = ((chapter_duration_ms / min_segment_ms).max(1)) as usize;
    if slides.len() > max_slides {
        slides.truncate(max_slides);
    }

    let total_weight: u64 = slides.iter().map(|(_, w)| *w).sum();
    let mut segments: Vec<ImageSegment> = Vec::with_capacity(slides.len());
    let mut acc_ms: u64 = 0;
    for (i, (src, weight)) in slides.iter().enumerate() {
        let span_ms = if i + 1 == slides.len() {
            // Last slide swallows rounding remainder so the total
            // matches chapter_duration_ms exactly — ffmpeg's concat
            // demuxer is unforgiving about A/V drift.
            chapter_duration_ms.saturating_sub(acc_ms)
        } else {
            ((*weight as u128 * chapter_duration_ms as u128) / total_weight as u128) as u64
        };
        segments.push(ImageSegment {
            image_src: src.clone(),
            duration_ms: span_ms,
        });
        acc_ms += span_ms;
    }
    segments
}

/// Encode a single MP4 whose video track is a slideshow of the given
/// image segments and whose audio track is the concatenation of the
/// listed WAVs. Image and audio streams are independent — the encoder
/// just needs them to add up to the same total length.
///
/// We pre-render each unique image source to a 1920×1080 composite
/// PNG (cached by source path), then invoke ffmpeg with one
/// `-loop 1 -t <d> -i <png>` input per image segment plus a `concat`
/// filter to splice them. The audio side uses the concat demuxer over
/// the wav list.
/// Concatenate per-chapter animated companion MP4s (produced by the
/// `animate` job) into a single book-wide MP4 with `-c copy` — no
/// re-encode, ~instant on multi-GB inputs because we're just rewriting
/// the container.
///
/// Inputs must already be in a uniform format (H.264 + AAC at the same
/// resolution + sample rate); the renderer enforces this. If they
/// drift, ffmpeg surfaces a stream-property mismatch and we surface
/// the error verbatim.
async fn concat_animated_chapters(
    state: &AppState,
    chapter_videos: &[PathBuf],
    out_path: &Path,
) -> Result<()> {
    let bin = state.config().ffmpeg_bin.trim();
    if bin.is_empty() {
        return Err(Error::Config("ffmpeg_bin is empty".into()));
    }
    if chapter_videos.is_empty() {
        return Err(Error::Conflict("no chapter videos to concat".into()));
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Other(anyhow::anyhow!("create yt out dir: {e}")))?;
    }

    // ffmpeg's `concat` demuxer expects a text file of `file '<path>'`
    // entries. Two gotchas the canonicalize step below handles:
    //   1. **Path resolution.** ffmpeg resolves *relative* entries
    //      against the *list file's directory*, not the process CWD.
    //      Our `chapter_videos` come from the storage builder as
    //      `./storage/audio/<id>/<lang>/ch-N.video.mp4` (relative to
    //      CWD) and the list itself lives at
    //      `./storage/audio/<id>/<lang>/youtube.concat.txt`, so a
    //      naive write produces `<list_dir>/./storage/audio/...` —
    //      doubled prefix, no such file.
    //   2. Single-quote escaping is rare in ffmpeg-friendly paths
    //      but we handle it for safety.
    // The chapter MP4s have already been existence-checked by the
    // caller, so canonicalize is safe (it requires existence).
    let list_path = out_path.with_extension("concat.txt");
    let mut list = String::new();
    for v in chapter_videos {
        let abs = std::fs::canonicalize(v).map_err(|e| {
            Error::Other(anyhow::anyhow!(
                "canonicalize chapter video {}: {e}",
                v.display()
            ))
        })?;
        let escaped = abs.display().to_string().replace('\'', r"'\''");
        list.push_str("file '");
        list.push_str(&escaped);
        list.push_str("'\n");
    }
    std::fs::write(&list_path, list)
        .map_err(|e| Error::Other(anyhow::anyhow!("write concat list: {e}")))?;

    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-y")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-c")
        .arg("copy")
        // Always re-mux into mp4 (the inputs are mp4 too, so this is
        // ~free). +faststart pulls the moov atom to the front so
        // YouTube can begin processing the upload before it's done.
        .arg("-movflags")
        .arg("+faststart")
        .arg(out_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = cmd
        .output()
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("spawn ffmpeg (animate concat): {e}")))?;

    // Best-effort cleanup; failure to unlink the list is not fatal.
    let _ = std::fs::remove_file(&list_path);

    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(Error::Other(anyhow::anyhow!(
            "ffmpeg (animate concat) exited with {}: {}",
            output.status,
            tail.trim_end()
        )));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct HyperframesCompositionRequest {
    id: String,
    title: String,
    subtitle: String,
    duration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HyperframesStatusResponse {
    status: String,
    #[serde(default)]
    error: Option<String>,
}

/// Map an explicit step request to an effective per-composition count.
/// `None` (or the integrations handler's "0/1 = auto" sentinel) →
/// auto-scale at ≈ 1 step / 15 s of narration, capped at 120 and floored
/// at the chapter count so every chapter still gets at least a beat.
fn effective_hyperframes_steps(steps: Option<usize>, total_ms: u64, chapter_count: usize) -> usize {
    let auto = (total_ms / 15_000).clamp(2, 120) as usize;
    let chosen = steps.unwrap_or(auto);
    chosen.max(chapter_count).clamp(2, 120)
}

#[allow(clippy::too_many_arguments)]
async fn render_hyperframes_video(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    audiobook_id: &str,
    language: &str,
    cover_path: &Path,
    total_ms: u64,
    steps: Option<usize>,
    vertical: bool,
    composition_suffix: &str,
    out_path: &Path,
) -> Result<()> {
    let cfg = state.config();
    let api_key = cfg.hyperframes_api_key.trim();
    if api_key.is_empty() {
        return Err(Error::Config(
            "hyperframes_api_key is not configured".into(),
        ));
    }
    let base = cfg.hyperframes_base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(Error::Config(
            "hyperframes_base_url is not configured".into(),
        ));
    }
    if chapters.is_empty() {
        return Err(Error::Conflict("no chapters for Hyperframes render".into()));
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Other(anyhow::anyhow!("create hyperframes output dir: {e}")))?;
    }
    ctx.progress(job, "hyperframes_composing", 0.10).await;

    // Bounded HTTP client. The per-request timeout has to swallow the
    // tail-end MP4 download, which scales with composition length —
    // at a conservative 5 Mbit/s a 30 min render can take a couple of
    // minutes to pull down. We give it 30 minutes here and rely on the
    // poll loop's wall-budget (below) plus the 10 s connect timeout to
    // detect a hung render server.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30 * 60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| Error::Upstream(format!("hyperframes http client: {e}")))?;

    // Render duration matches narration length (rounded up). Vertical
    // = Shorts, capped at YouTube's 180 s upper bound. Landscape =
    // long-form audiobook, capped at one hour so a misconfigured book
    // can't queue a multi-hour render against a shared service.
    let max_secs: u64 = if vertical { 180 } else { 60 * 60 };
    let duration_secs = total_ms.max(1).div_ceil(1000).clamp(5, max_secs);
    let effective_steps = effective_hyperframes_steps(steps, total_ms, chapters.len());
    // Scope the composition id by audiobook + language + suffix so
    // failures on the shared Hyperframes server are easier to trace
    // back to a publication, and per-chapter playlist renders don't
    // collide with one another.
    let safe_lang = language.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let safe_book = audiobook_id.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let safe_suffix = composition_suffix.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let id = format!(
        "aidbooks-{safe_book}-{safe_lang}-{safe_suffix}-{}",
        uuid::Uuid::new_v4().simple()
    );
    let storage_root = state.config().storage_path.as_path();
    let html = if vertical {
        build_hyperframes_html_short(
            &id,
            book,
            chapters,
            cover_path,
            storage_root,
            effective_steps,
            duration_secs,
        )?
    } else {
        build_hyperframes_html_long(
            &id,
            book,
            chapters,
            cover_path,
            storage_root,
            effective_steps,
            duration_secs,
        )?
    };
    let body = HyperframesCompositionRequest {
        id: id.clone(),
        title: book.title.clone(),
        subtitle: String::new(),
        duration: duration_secs,
        html: Some(html),
    };

    tracing::info!(
        composition_id = %id,
        duration_secs,
        steps = effective_steps,
        chapters = chapters.len(),
        vertical,
        "hyperframes: creating composition"
    );

    let composition_url = format!("{base}/compositions");
    let res = client
        .post(&composition_url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("hyperframes create composition: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(Error::Upstream(format!(
            "hyperframes create composition failed {status}: {text}"
        )));
    }

    ctx.progress(job, "hyperframes_render_started", 0.15).await;
    let render_url = format!("{base}/render/{id}");
    let res = client
        .post(&render_url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("hyperframes start render: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let _ = delete_hyperframes_composition(&client, base, api_key, &id).await;
        return Err(Error::Upstream(format!(
            "hyperframes start render failed {status}: {text}"
        )));
    }

    let status_url = format!("{base}/status/{id}");
    // The render server captures the page frame-by-frame in headless
    // Chromium and re-encodes with ffmpeg, so wall-clock time scales
    // with the composition duration rather than being a fixed cap. We
    // budget ≈ 6× real-time (Shorts: 90 s narration → 9 min budget;
    // 30 min book → 3 hr budget) and clamp the upper bound at 4 hours
    // so a stuck render still fails the job eventually instead of
    // pinning a worker forever.
    let render_budget = Duration::from_secs((duration_secs * 6).clamp(8 * 60, 4 * 60 * 60));
    // Polling cadence backs off for long renders — there's no point
    // hammering a 1-hour render every two seconds.
    let poll_interval = if duration_secs <= 180 {
        Duration::from_secs(2)
    } else if duration_secs <= 30 * 60 {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(10)
    };
    let started = std::time::Instant::now();
    let mut completed = false;
    let mut attempt: u64 = 0;
    while started.elapsed() < render_budget {
        attempt += 1;
        tokio::time::sleep(poll_interval).await;
        let res = match client.get(&status_url).bearer_auth(api_key).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "hyperframes status request failed; will retry");
                continue;
            }
        };
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let _ = delete_hyperframes_composition(&client, base, api_key, &id).await;
            return Err(Error::Upstream(format!(
                "hyperframes status failed {status}: {text}"
            )));
        }
        let status: HyperframesStatusResponse = res
            .json()
            .await
            .map_err(|e| Error::Upstream(format!("hyperframes status decode: {e}")))?;
        match status.status.as_str() {
            "completed" => {
                completed = true;
                break;
            }
            "failed" => {
                let upstream_err = status.error.unwrap_or_else(|| "unknown error".into());
                tracing::warn!(
                    composition_id = %id,
                    upstream_error = %upstream_err,
                    elapsed_secs = started.elapsed().as_secs(),
                    "hyperframes: render server returned failed"
                );
                let _ = delete_hyperframes_composition(&client, base, api_key, &id).await;
                return Err(Error::Upstream(format!(
                    "hyperframes render failed: {upstream_err}"
                )));
            }
            _ => {
                // Map elapsed/budget onto [0.15..0.80] so the bar
                // creeps forward steadily even on multi-hour renders.
                let elapsed_frac =
                    (started.elapsed().as_secs_f32() / render_budget.as_secs_f32()).clamp(0.0, 1.0);
                let pct = 0.15 + 0.65 * elapsed_frac;
                ctx.progress(job, "hyperframes_rendering", pct).await;
                if attempt.is_multiple_of(30) {
                    tracing::info!(
                        composition_id = %id,
                        elapsed_secs = started.elapsed().as_secs(),
                        budget_secs = render_budget.as_secs(),
                        "hyperframes: still rendering"
                    );
                }
                continue;
            }
        }
    }
    if !completed {
        let _ = delete_hyperframes_composition(&client, base, api_key, &id).await;
        return Err(Error::Upstream(format!(
            "hyperframes render timed out after {} s waiting for completion (composition duration {duration_secs} s)",
            render_budget.as_secs()
        )));
    }

    ctx.progress(job, "hyperframes_downloading", 0.85).await;
    // Suffix-scoped filename so per-chapter renders in playlist mode
    // don't clobber one another's intermediate file.
    let visual_path =
        out_path.with_file_name(format!("youtube.hyperframes.visual-{safe_suffix}.mp4"));
    let download_url = format!("{base}/download/{id}");
    let bytes = client
        .get(&download_url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("hyperframes download: {e}")))?
        .error_for_status()
        .map_err(|e| Error::Upstream(format!("hyperframes download status: {e}")))?
        .bytes()
        .await
        .map_err(|e| Error::Upstream(format!("hyperframes download bytes: {e}")))?;
    tracing::info!(
        composition_id = %id,
        bytes = bytes.len(),
        "hyperframes: downloaded visual"
    );
    tokio::fs::write(&visual_path, bytes)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("write hyperframes visual: {e}")))?;

    ctx.progress(job, "hyperframes_muxing", 0.92).await;
    let mux_result =
        mux_hyperframes_visual_with_audio(state, chapters, &visual_path, out_path, !vertical).await;

    // Best-effort cleanup regardless of mux outcome — composition stays on
    // the shared global server otherwise. Failures are logged, not bubbled.
    let _ = delete_hyperframes_composition(&client, base, api_key, &id).await;
    let _ = std::fs::remove_file(&visual_path);

    mux_result?;
    ctx.progress(job, "hyperframes_done", 0.95).await;
    Ok(())
}

/// Long-form Hyperframes pipeline: render one composition per chapter,
/// then ffmpeg-concat into a single MP4.
///
/// The shared Hyperframes render server has its own internal time
/// budget per composition (empirically ~5 minutes wall-clock); a single
/// composition for an entire 30-minute audiobook reliably hits that
/// limit and comes back as `status:"failed"`. By splitting the book at
/// chapter boundaries we keep each upstream request small enough to
/// finish, at the cost of one extra concat pass on our side.
///
/// User-requested `hyperframes_steps` is treated as a *book-level*
/// total when present and allocated across chapters in proportion to
/// their narration duration; `None` lets each chapter auto-scale on
/// its own duration.
#[allow(clippy::too_many_arguments)]
async fn render_hyperframes_long_chunked(
    state: &AppState,
    ctx: &JobContext,
    job: &JobRow,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    audiobook_id: &str,
    language: &str,
    cover_path: &Path,
    total_steps: Option<usize>,
    out_path: &Path,
) -> Result<()> {
    if chapters.is_empty() {
        return Err(Error::Conflict("no chapters for Hyperframes render".into()));
    }

    let chapter_alloc: Option<Vec<usize>> = total_steps.map(|steps| {
        let chosen = steps.max(chapters.len()).clamp(2, 120);
        allocate_steps(chapters, chosen)
    });

    let dir = out_path
        .parent()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("youtube output path has no parent")))?
        .to_path_buf();
    let mut chunk_paths: Vec<PathBuf> = Vec::with_capacity(chapters.len());
    let n = chapters.len();
    for (idx, ch) in chapters.iter().enumerate() {
        let span_start = 0.10 + (idx as f32 / n as f32) * 0.75;
        let span_end = 0.10 + ((idx + 1) as f32 / n as f32) * 0.75;
        ctx.progress(
            job,
            &format!("hyperframes ch{}/{}", idx + 1, n),
            span_start.clamp(0.0, 0.99),
        )
        .await;

        let chunk_path = dir.join(format!("youtube.hyperframes.ch-{}.mp4", ch.number));
        let chapter_total_ms = (ch.duration_ms.unwrap_or(0).max(0) as u64).max(1);
        let chapter_steps = chapter_alloc.as_ref().and_then(|a| a.get(idx).copied());
        let suffix = format!("ch-{}", ch.number);

        if let Err(e) = render_hyperframes_video(
            state,
            ctx,
            job,
            book,
            std::slice::from_ref(ch),
            audiobook_id,
            language,
            cover_path,
            chapter_total_ms,
            chapter_steps,
            false,
            &suffix,
            &chunk_path,
        )
        .await
        {
            // Best-effort cleanup of any chunks we did manage to render.
            for p in &chunk_paths {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
        chunk_paths.push(chunk_path);
        ctx.progress(
            job,
            &format!("hyperframes ch{}/{} done", idx + 1, n),
            span_end.clamp(0.0, 0.99),
        )
        .await;
    }

    if chunk_paths.len() == 1 {
        // Single-chapter book: just rename the chunk to the final
        // output path; no concat pass needed.
        std::fs::rename(&chunk_paths[0], out_path).map_err(|e| {
            Error::Other(anyhow::anyhow!("rename hyperframes chunk to output: {e}"))
        })?;
    } else {
        ctx.progress(job, "hyperframes_concat", 0.86).await;
        concat_hyperframes_chunks(state, &chunk_paths, out_path).await?;
        for p in &chunk_paths {
            let _ = std::fs::remove_file(p);
        }
    }
    ctx.progress(job, "hyperframes_done", 0.90).await;
    Ok(())
}

/// ffmpeg `concat` demuxer pass — stitches per-chapter Hyperframes
/// MP4s into a single output without re-encoding (`-c copy`). All
/// inputs share the same H.264/AAC profile because they all come out
/// of `mux_hyperframes_visual_with_audio`, so concat-copy is safe.
async fn concat_hyperframes_chunks(
    state: &AppState,
    chunks: &[PathBuf],
    out_path: &Path,
) -> Result<()> {
    let bin = state.config().ffmpeg_bin.trim();
    if bin.is_empty() {
        return Err(Error::Config("ffmpeg_bin is not configured".into()));
    }
    let list_path = out_path.with_extension("hyperframes-chunks.txt");
    let mut list = String::new();
    for p in chunks {
        let abs = std::fs::canonicalize(p).map_err(|e| {
            Error::Other(anyhow::anyhow!("canonicalize chunk {}: {e}", p.display()))
        })?;
        let escaped = abs.display().to_string().replace('\'', r"'\''");
        list.push_str("file '");
        list.push_str(&escaped);
        list.push_str("'\n");
    }
    std::fs::write(&list_path, list)
        .map_err(|e| Error::Other(anyhow::anyhow!("write hyperframes chunk list: {e}")))?;

    let output = tokio::process::Command::new(bin)
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("warning")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-c")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(out_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("spawn ffmpeg (hyperframes concat): {e}")))?;
    let _ = std::fs::remove_file(&list_path);
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(Error::Other(anyhow::anyhow!(
            "ffmpeg (hyperframes concat) exited with {}: {}",
            output.status,
            tail.trim_end()
        )));
    }
    Ok(())
}

async fn delete_hyperframes_composition(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    id: &str,
) -> Result<()> {
    let url = format!("{base}/compositions/{id}");
    let res = client
        .delete(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| Error::Upstream(format!("hyperframes delete: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        tracing::warn!(
            composition_id = %id,
            %status,
            body = %text,
            "hyperframes delete returned non-success"
        );
    } else {
        tracing::info!(composition_id = %id, "hyperframes: composition deleted");
    }
    Ok(())
}

/// Build a content-aware HTML composition for a YouTube Shorts narration.
///
/// Each chapter gets one or more "scenes" allocated proportionally to its
/// audio duration (largest-remainder method, with at least one scene per
/// chapter; if `steps < chapters.len()` the effective step count rises to
/// `chapters.len()`). Each scene shows a chapter image (chapter art →
/// paragraph illustration → cover fallback, in that order) with a Ken-Burns
/// zoom and an animated chapter-title / tagline overlay. The tagline cycles
/// through sentences extracted from the chapter synopsis (or body when
/// synopsis is missing).
///
/// All images are inlined as base64 `data:` URLs so the public Hyperframes
/// renderer can fetch them without seeing local storage. Images larger than
/// `MAX_IMAGE_INLINE_BYTES` are skipped and the scene falls back to a
/// gradient — keeps the request body manageable.
fn build_hyperframes_html_short(
    composition_id: &str,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    cover_path: &Path,
    storage_root: &Path,
    steps: usize,
    total_secs: u64,
) -> Result<String> {
    if chapters.is_empty() {
        return Err(Error::Conflict("no chapters for Hyperframes render".into()));
    }

    let alloc = allocate_steps(chapters, steps);

    struct SceneSpec {
        start_secs: f64,
        duration_secs: f64,
        image_data_url: Option<String>,
        ch_num: i64,
        ch_title: String,
        tagline: String,
        is_first: bool,
    }
    let mut scenes: Vec<SceneSpec> = Vec::new();
    let mut t_acc_ms: u64 = 0;
    for (ci, ch) in chapters.iter().enumerate() {
        let n = alloc.get(ci).copied().unwrap_or(1).max(1);
        let dur_ms = (ch.duration_ms.unwrap_or(0).max(0) as u64).max(1);

        let mut images: Vec<PathBuf> = Vec::new();
        if let Some(rel) = ch.chapter_art_path.as_deref() {
            let p = storage_root.join(rel);
            if p.exists() {
                images.push(p);
            }
        }
        if let Some(ps) = ch.paragraphs.as_ref() {
            for p in ps {
                for rel in &p.image_paths {
                    if rel.trim().is_empty() {
                        continue;
                    }
                    let abs = storage_root.join(rel);
                    if abs.exists() && !images.iter().any(|x| x == &abs) {
                        images.push(abs);
                    }
                }
            }
        }
        if images.is_empty() {
            images.push(cover_path.to_path_buf());
        }

        let synopsis_src = ch.synopsis.as_deref().unwrap_or("");
        let body_src = ch.body_md.as_deref().unwrap_or("");
        let mut sentences = split_into_sentences(synopsis_src, n);
        if sentences.len() < n {
            let need = n - sentences.len();
            sentences.extend(split_into_sentences(body_src, need));
        }

        let scene_dur_ms = dur_ms / n as u64;
        for k in 0..n {
            let img_path = &images[k % images.len()];
            let data_url = read_image_as_data_url(img_path, MAX_IMAGE_INLINE_BYTES);
            let this_scene_ms = if k + 1 == n {
                dur_ms - scene_dur_ms * (n as u64 - 1)
            } else {
                scene_dur_ms
            };
            let start_secs = (t_acc_ms as f64) / 1000.0;
            let dur_secs = (this_scene_ms as f64) / 1000.0;
            let tagline = sentences
                .get(k)
                .map(|s| truncate_sentence(s, 140))
                .unwrap_or_default();
            scenes.push(SceneSpec {
                start_secs,
                duration_secs: dur_secs,
                image_data_url: data_url,
                ch_num: ch.number,
                ch_title: ch.title.clone(),
                tagline,
                is_first: scenes.is_empty(),
            });
            t_acc_ms += this_scene_ms;
        }
    }

    let book_title_html = html_escape(&book.title);

    let mut scene_html = String::new();
    let mut tween_js = String::new();
    for (i, sc) in scenes.iter().enumerate() {
        let bg_css = match &sc.image_data_url {
            Some(url) => {
                let safe = url.replace('\'', "%27");
                format!("background-image:url('{safe}')")
            }
            None => {
                "background:linear-gradient(135deg,#1a2f5a 0%,#2e1a5a 50%,#5a1a3f 100%)".to_string()
            }
        };
        let ch_label = html_escape(&format!("Chapter {}", sc.ch_num));
        let title_html = html_escape(&sc.ch_title);
        let tagline_html = html_escape(&sc.tagline);
        let opener_html = if sc.is_first {
            format!(r#"<div class="opener">{}</div>"#, book_title_html)
        } else {
            String::new()
        };
        scene_html.push_str(&format!(
            r#"<div class="scene" id="sc-{i}"><div class="img" style="{bg_css}"></div><div class="vignette"></div><div class="gradient"></div><div class="ch-num">{ch_label}</div><div class="title">{title_html}</div>{opener_html}<div class="tagline">{tagline_html}</div></div>"#
        ));

        let t_start = sc.start_secs;
        let t_dur = sc.duration_secs.max(0.4);
        let t_in = (t_dur * 0.20).min(0.5);
        let t_out_at = (t_start + t_dur - t_in).max(t_start);
        let t_text_in = t_start + t_in.min(0.15);
        let t_tagline_in = t_start + t_in.min(0.30);
        tween_js.push_str(&format!(
            "tl.fromTo('#sc-{i}',{{opacity:0}},{{opacity:1,duration:{t_in:.3}}},{t_start:.3});\n"
        ));
        tween_js.push_str(&format!(
            "tl.fromTo('#sc-{i} .img',{{scale:1.0,xPercent:0}},{{scale:1.12,xPercent:-2,duration:{t_dur:.3},ease:'none'}},{t_start:.3});\n"
        ));
        tween_js.push_str(&format!(
            "tl.fromTo('#sc-{i} .title',{{y:40,opacity:0}},{{y:0,opacity:1,duration:0.7,ease:'power2.out'}},{t_text_in:.3});\n"
        ));
        tween_js.push_str(&format!(
            "tl.fromTo('#sc-{i} .tagline',{{y:30,opacity:0}},{{y:0,opacity:1,duration:0.6,ease:'power2.out'}},{t_tagline_in:.3});\n"
        ));
        if sc.is_first {
            let t_opener_out = t_start + t_dur * 0.6;
            tween_js.push_str(&format!(
                "tl.fromTo('#sc-{i} .opener',{{y:60,opacity:0}},{{y:0,opacity:1,duration:0.9,ease:'power3.out'}},{t_start:.3});\n"
            ));
            tween_js.push_str(&format!(
                "tl.to('#sc-{i} .opener',{{opacity:0,duration:0.5,ease:'power1.in'}},{t_opener_out:.3});\n"
            ));
        }
        tween_js.push_str(&format!(
            "tl.to('#sc-{i}',{{opacity:0,duration:{t_in:.3},ease:'power1.in'}},{t_out_at:.3});\n"
        ));
    }

    let total_secs_f = total_secs as f64;
    let html = format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>{book_title_html}</title>
<script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js"></script>
<style>
html,body{{margin:0;padding:0;background:#000;font-family:'Helvetica Neue',Helvetica,Arial,sans-serif;color:#fff;overflow:hidden}}
#stage{{position:relative;width:1080px;height:1920px;overflow:hidden;background:#000}}
.scene{{position:absolute;inset:0;opacity:0;will-change:opacity}}
.scene .img{{position:absolute;inset:-4%;background-size:cover;background-position:center;will-change:transform;transform-origin:center center}}
.scene .vignette{{position:absolute;inset:0;background:radial-gradient(ellipse at center,transparent 35%,rgba(0,0,0,0.55) 100%);pointer-events:none}}
.scene .gradient{{position:absolute;left:0;right:0;bottom:0;height:60%;background:linear-gradient(180deg,transparent 0%,rgba(0,0,0,0.85) 100%);pointer-events:none}}
.scene .ch-num{{position:absolute;left:60px;top:120px;font-size:32px;letter-spacing:8px;color:#9bd6ff;text-transform:uppercase;font-weight:600;text-shadow:0 2px 8px rgba(0,0,0,0.6)}}
.scene .title{{position:absolute;left:60px;right:60px;bottom:480px;font-size:96px;font-weight:800;line-height:1.05;letter-spacing:-1px;text-shadow:0 4px 20px rgba(0,0,0,0.7)}}
.scene .tagline{{position:absolute;left:60px;right:60px;bottom:240px;font-size:42px;font-weight:300;line-height:1.3;color:#dfe5f0;text-shadow:0 2px 12px rgba(0,0,0,0.7)}}
.scene .opener{{position:absolute;left:60px;right:60px;top:50%;transform:translateY(-50%);text-align:center;font-size:120px;font-weight:800;letter-spacing:-2px;line-height:1.0;background:linear-gradient(120deg,#6ea8ff 0%,#c08bff 50%,#ffb86b 100%);-webkit-background-clip:text;background-clip:text;color:transparent}}
.progress-bar{{position:absolute;left:0;right:0;bottom:0;height:6px;background:rgba(255,255,255,0.12)}}
.progress-bar > .fill{{position:absolute;left:0;top:0;bottom:0;width:0;background:linear-gradient(90deg,#6ea8ff,#c08bff,#ffb86b)}}
</style></head><body>
<div id="stage" data-composition-id="{composition_id}" data-start="0" data-duration="{total_secs}" data-track-index="0">
{scene_html}
<div class="progress-bar"><div class="fill"></div></div>
</div>
<script>
window.__timelines = window.__timelines || {{}};
(function(){{
  var tl = gsap.timeline({{paused:true}});
{tween_js}
  tl.fromTo('.progress-bar > .fill',{{width:'0%'}},{{width:'100%',duration:{total_secs_f},ease:'none'}},0);
  window.__timelines['{composition_id}'] = tl;
}})();
</script>
</body></html>"##
    );

    Ok(html)
}

/// Build the long-form (16:9 landscape) Hyperframes composition.
///
/// The Hyperframes render server is hard-wired to a 1080×1920 portrait
/// viewport (it ignores width/height/aspect_ratio fields on the
/// composition request, see `memory/project_hyperframes_api.md`). To
/// produce a true 1920×1080 landscape MP4 we instead author the HTML
/// with a 1920×1080 inner canvas rotated -90° inside the portrait
/// viewport; the renderer captures content sideways at 1080×1920 and
/// the mux step then applies `ffmpeg -vf transpose=1` to rotate the
/// output upright.
///
/// Compared to the Shorts builder, the scene plan picks from four
/// distinct kinds — title card (book-level opener), chapter-intro
/// card (large chapter number + title at the start of every chapter),
/// image-with-caption (the staple), and pull-quote (sentence-only
/// card on a gradient background, sprinkled in for rhythm). Each
/// scene gets its own GSAP tween block; sentences come from the
/// chapter synopsis falling back to body prose, identical to the
/// Shorts pass.
fn build_hyperframes_html_long(
    composition_id: &str,
    book: &DbAudiobook,
    chapters: &[DbChapter],
    cover_path: &Path,
    storage_root: &Path,
    steps: usize,
    total_secs: u64,
) -> Result<String> {
    if chapters.is_empty() {
        return Err(Error::Conflict("no chapters for Hyperframes render".into()));
    }

    // Scene-slot plan — title card + per-chapter intro + body slots.
    // Per-chapter rendering (playlist mode passes chapters.len()==1)
    // skips the book title card so each chapter video opens with its
    // own intro instead of the same book splash.
    let include_title_card = chapters.len() > 1;
    let title_slots: usize = if include_title_card { 1 } else { 0 };
    let intro_slots = chapters.len();
    let mut body_slots = steps.saturating_sub(title_slots + intro_slots);
    // Always leave at least one body scene per chapter so a low `steps`
    // value doesn't strip the chapter prose visuals entirely.
    if body_slots < chapters.len() {
        body_slots = chapters.len();
    }
    let body_alloc = allocate_steps(chapters, body_slots);

    // Resolve images per chapter (chapter art → paragraph art → cover).
    let mut chapter_images: Vec<Vec<PathBuf>> = Vec::with_capacity(chapters.len());
    for ch in chapters {
        let mut images: Vec<PathBuf> = Vec::new();
        if let Some(rel) = ch.chapter_art_path.as_deref() {
            let p = storage_root.join(rel);
            if p.exists() {
                images.push(p);
            }
        }
        if let Some(ps) = ch.paragraphs.as_ref() {
            for p in ps {
                for rel in &p.image_paths {
                    if rel.trim().is_empty() {
                        continue;
                    }
                    let abs = storage_root.join(rel);
                    if abs.exists() && !images.iter().any(|x| x == &abs) {
                        images.push(abs);
                    }
                }
            }
        }
        if images.is_empty() {
            images.push(cover_path.to_path_buf());
        }
        chapter_images.push(images);
    }

    enum Kind {
        TitleCard,
        ChapterIntro,
        ImageCaption,
        PullQuote,
    }
    struct SceneSpec {
        kind: Kind,
        start_secs: f64,
        duration_secs: f64,
        image_data_url: Option<String>,
        ch_num: i64,
        ch_title: String,
        text: String,
    }

    let mut scenes: Vec<SceneSpec> = Vec::new();
    let mut t_acc_ms: u64 = 0;

    // Time budget: a TitleCard sits at the very top and steals from the
    // first chapter; ChapterIntros sit inside their chapter's window.
    // Both have a fixed target with chapter-proportional caps so a
    // very short chapter can't leave 0 ms for body scenes.
    const TITLE_CARD_TARGET_MS: u64 = 3_500;
    const CHAPTER_INTRO_TARGET_MS: u64 = 2_500;

    for (ci, ch) in chapters.iter().enumerate() {
        let ch_dur_ms = (ch.duration_ms.unwrap_or(0).max(0) as u64).max(1);
        let body_n = body_alloc.get(ci).copied().unwrap_or(1).max(1);

        // Reserve title-card duration only on the first chapter.
        let title_ms = if ci == 0 && include_title_card {
            TITLE_CARD_TARGET_MS.min(ch_dur_ms / 4)
        } else {
            0
        };
        // Chapter intro: don't eat more than ~40 % of the chapter on a
        // very short chapter, and never more than the target.
        let intro_ms = CHAPTER_INTRO_TARGET_MS.min((ch_dur_ms - title_ms) * 2 / 5);
        let body_total_ms = ch_dur_ms - title_ms - intro_ms;
        let body_each_ms = if body_n > 0 {
            body_total_ms / body_n as u64
        } else {
            body_total_ms
        };

        // Pull body sentences first from synopsis, padded with prose.
        let synopsis_src = ch.synopsis.as_deref().unwrap_or("");
        let body_src = ch.body_md.as_deref().unwrap_or("");
        let mut sentences = split_into_sentences(synopsis_src, body_n + 2);
        if sentences.len() < body_n + 2 {
            let need = body_n + 2 - sentences.len();
            sentences.extend(split_into_sentences(body_src, need));
        }

        // Scene 1: title card (only on the first chapter, only when we
        // have more than one chapter).
        if title_ms > 0 {
            scenes.push(SceneSpec {
                kind: Kind::TitleCard,
                start_secs: (t_acc_ms as f64) / 1000.0,
                duration_secs: (title_ms as f64) / 1000.0,
                image_data_url: read_image_as_data_url(cover_path, MAX_IMAGE_INLINE_BYTES),
                ch_num: ch.number,
                ch_title: book.title.clone(),
                text: String::new(),
            });
            t_acc_ms += title_ms;
        }

        // Scene 2: chapter intro.
        let intro_image = chapter_images[ci]
            .first()
            .cloned()
            .unwrap_or_else(|| cover_path.to_path_buf());
        scenes.push(SceneSpec {
            kind: Kind::ChapterIntro,
            start_secs: (t_acc_ms as f64) / 1000.0,
            duration_secs: (intro_ms as f64) / 1000.0,
            image_data_url: read_image_as_data_url(&intro_image, MAX_IMAGE_INLINE_BYTES),
            ch_num: ch.number,
            ch_title: ch.title.clone(),
            text: ch.synopsis.clone().unwrap_or_default(),
        });
        t_acc_ms += intro_ms;

        // Scenes 3..: body. Cycle through chapter images, sprinkle one
        // pull-quote roughly every 4th body scene (offset so different
        // chapters don't all land their quote in the same slot).
        for k in 0..body_n {
            let this_ms = if k + 1 == body_n {
                body_total_ms - body_each_ms * (body_n as u64 - 1)
            } else {
                body_each_ms
            };
            let kind = if body_n >= 4 && (k + 1 + ci) % 4 == 0 {
                Kind::PullQuote
            } else {
                Kind::ImageCaption
            };
            let img_path = &chapter_images[ci][k % chapter_images[ci].len()];
            let data_url = match &kind {
                Kind::PullQuote => None,
                _ => read_image_as_data_url(img_path, MAX_IMAGE_INLINE_BYTES),
            };
            let text = sentences
                .get(k)
                .map(|s| {
                    truncate_sentence(
                        s,
                        match &kind {
                            Kind::PullQuote => 220,
                            _ => 160,
                        },
                    )
                })
                .unwrap_or_default();
            scenes.push(SceneSpec {
                kind,
                start_secs: (t_acc_ms as f64) / 1000.0,
                duration_secs: (this_ms as f64) / 1000.0,
                image_data_url: data_url,
                ch_num: ch.number,
                ch_title: ch.title.clone(),
                text,
            });
            t_acc_ms += this_ms;
        }
    }

    let book_title_html = html_escape(&book.title);

    let mut scene_html = String::new();
    let mut tween_js = String::new();
    for (i, sc) in scenes.iter().enumerate() {
        let title_html = html_escape(&sc.ch_title);
        let text_html = html_escape(&sc.text);
        let ch_label = html_escape(&format!("Chapter {}", sc.ch_num));
        let bg_css = match (&sc.kind, &sc.image_data_url) {
            (Kind::PullQuote, _) => {
                "background:linear-gradient(135deg,#1a2f5a 0%,#2e1a5a 50%,#5a1a3f 100%)".to_string()
            }
            (_, Some(url)) => {
                let safe = url.replace('\'', "%27");
                format!("background-image:url('{safe}')")
            }
            (_, None) => {
                "background:linear-gradient(135deg,#1a2f5a 0%,#2e1a5a 50%,#5a1a3f 100%)".to_string()
            }
        };

        let scene_body = match &sc.kind {
            Kind::TitleCard => format!(
                r#"<div class="img" style="{bg_css}"></div><div class="vignette"></div><div class="scrim"></div><div class="title-card-eyebrow">{book}</div><div class="title-card-main">{title}</div>"#,
                book = html_escape("Audiobook"),
                title = book_title_html,
                bg_css = bg_css,
            ),
            Kind::ChapterIntro => format!(
                r#"<div class="img" style="{bg_css}"></div><div class="vignette"></div><div class="scrim"></div><div class="intro-eyebrow">{ch_label}</div><div class="intro-title">{title}</div>"#,
                ch_label = ch_label,
                title = title_html,
                bg_css = bg_css,
            ),
            Kind::ImageCaption => format!(
                r#"<div class="img" style="{bg_css}"></div><div class="vignette"></div><div class="gradient"></div><div class="ch-num">{ch_label}</div><div class="title">{title}</div><div class="tagline">{text}</div>"#,
                ch_label = ch_label,
                title = title_html,
                text = text_html,
                bg_css = bg_css,
            ),
            Kind::PullQuote => format!(
                r#"<div class="img" style="{bg_css}"></div><div class="quote-mark">&ldquo;</div><div class="quote">{text}</div><div class="quote-source">{ch_label}</div>"#,
                ch_label = ch_label,
                text = text_html,
                bg_css = bg_css,
            ),
        };
        scene_html.push_str(&format!(
            r#"<div class="scene scene-{kind_class}" id="sc-{i}">{body}</div>"#,
            kind_class = match &sc.kind {
                Kind::TitleCard => "title",
                Kind::ChapterIntro => "intro",
                Kind::ImageCaption => "img",
                Kind::PullQuote => "quote",
            },
            body = scene_body,
        ));

        let t_start = sc.start_secs;
        let t_dur = sc.duration_secs.max(0.4);
        let t_in = (t_dur * 0.20).min(0.5);
        let t_out_at = (t_start + t_dur - t_in).max(t_start);
        tween_js.push_str(&format!(
            "tl.fromTo('#sc-{i}',{{opacity:0}},{{opacity:1,duration:{t_in:.3}}},{t_start:.3});\n"
        ));
        // Slow Ken-Burns on every scene that has an image. Pull-quote
        // and title cards keep their gradient background still.
        match &sc.kind {
            Kind::TitleCard => {
                let t_text_in = t_start + t_in.min(0.30);
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .img',{{scale:1.0}},{{scale:1.05,duration:{t_dur:.3},ease:'none'}},{t_start:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .title-card-eyebrow',{{y:24,opacity:0}},{{y:0,opacity:1,duration:0.5,ease:'power2.out'}},{t_start:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .title-card-main',{{y:60,opacity:0}},{{y:0,opacity:1,duration:0.9,ease:'power3.out'}},{t_text_in:.3});\n"
                ));
            }
            Kind::ChapterIntro => {
                let t_text_in = t_start + t_in.min(0.20);
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .img',{{scale:1.0,xPercent:0}},{{scale:1.10,xPercent:-1.5,duration:{t_dur:.3},ease:'none'}},{t_start:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .intro-eyebrow',{{y:30,opacity:0}},{{y:0,opacity:1,duration:0.6,ease:'power2.out'}},{t_start:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .intro-title',{{y:60,opacity:0}},{{y:0,opacity:1,duration:0.8,ease:'power3.out'}},{t_text_in:.3});\n"
                ));
            }
            Kind::ImageCaption => {
                let t_text_in = t_start + t_in.min(0.15);
                let t_tag_in = t_start + t_in.min(0.30);
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .img',{{scale:1.0,xPercent:0}},{{scale:1.12,xPercent:-2,duration:{t_dur:.3},ease:'none'}},{t_start:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .title',{{y:30,opacity:0}},{{y:0,opacity:1,duration:0.6,ease:'power2.out'}},{t_text_in:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .tagline',{{y:24,opacity:0}},{{y:0,opacity:1,duration:0.6,ease:'power2.out'}},{t_tag_in:.3});\n"
                ));
            }
            Kind::PullQuote => {
                let t_quote_in = t_start + t_in.min(0.25);
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .quote-mark',{{scale:0.8,opacity:0}},{{scale:1,opacity:1,duration:0.6,ease:'back.out(1.6)'}},{t_start:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .quote',{{y:40,opacity:0}},{{y:0,opacity:1,duration:0.8,ease:'power3.out'}},{t_quote_in:.3});\n"
                ));
                tween_js.push_str(&format!(
                    "tl.fromTo('#sc-{i} .quote-source',{{y:18,opacity:0}},{{y:0,opacity:1,duration:0.5,ease:'power2.out'}},{t_quote_in:.3});\n"
                ));
            }
        }
        tween_js.push_str(&format!(
            "tl.to('#sc-{i}',{{opacity:0,duration:{t_in:.3},ease:'power1.in'}},{t_out_at:.3});\n"
        ));
    }

    let total_secs_f = total_secs as f64;
    // The Hyperframes server captures the page at 1080×1920 portrait
    // (see project_hyperframes_api memory). The outer `#stage` keeps
    // those dimensions; the inner `#scene-canvas` is sized 1920×1080
    // and rotated -90° so scenes lay out in a landscape coordinate
    // system. The mux step downstream applies `ffmpeg transpose=1`
    // which rotates the captured MP4 by +90° to land back at upright
    // 1920×1080 — net rotation cancels.
    let html = format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>{book_title_html}</title>
<script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js"></script>
<style>
html,body{{margin:0;padding:0;background:#000;font-family:'Helvetica Neue',Helvetica,Arial,sans-serif;color:#fff;overflow:hidden}}
#stage{{position:relative;width:1080px;height:1920px;overflow:hidden;background:#000}}
#scene-canvas{{position:absolute;top:50%;left:50%;width:1920px;height:1080px;transform:translate(-50%,-50%) rotate(-90deg);transform-origin:center center}}
.scene{{position:absolute;inset:0;opacity:0;will-change:opacity}}
.scene .img{{position:absolute;inset:-4%;background-size:cover;background-position:center;will-change:transform;transform-origin:center center}}
.scene .vignette{{position:absolute;inset:0;background:radial-gradient(ellipse at center,transparent 35%,rgba(0,0,0,0.55) 100%);pointer-events:none}}
.scene .gradient{{position:absolute;left:0;right:0;bottom:0;height:55%;background:linear-gradient(180deg,transparent 0%,rgba(0,0,0,0.85) 100%);pointer-events:none}}
.scene .scrim{{position:absolute;inset:0;background:rgba(0,0,0,0.45);pointer-events:none}}
.scene .ch-num{{position:absolute;left:80px;top:80px;font-size:30px;letter-spacing:8px;color:#9bd6ff;text-transform:uppercase;font-weight:600;text-shadow:0 2px 8px rgba(0,0,0,0.6)}}
.scene .title{{position:absolute;left:80px;right:80px;bottom:240px;font-size:84px;font-weight:800;line-height:1.05;letter-spacing:-1px;text-shadow:0 4px 20px rgba(0,0,0,0.7)}}
.scene .tagline{{position:absolute;left:80px;right:80px;bottom:120px;font-size:36px;font-weight:300;line-height:1.3;color:#dfe5f0;text-shadow:0 2px 12px rgba(0,0,0,0.7)}}
.scene-title .title-card-eyebrow{{position:absolute;left:0;right:0;top:38%;text-align:center;font-size:28px;letter-spacing:14px;color:#9bd6ff;text-transform:uppercase;font-weight:600}}
.scene-title .title-card-main{{position:absolute;left:80px;right:80px;top:50%;transform:translateY(-50%);text-align:center;font-size:140px;font-weight:800;letter-spacing:-2px;line-height:1.0;background:linear-gradient(120deg,#6ea8ff 0%,#c08bff 50%,#ffb86b 100%);-webkit-background-clip:text;background-clip:text;color:transparent}}
.scene-intro .intro-eyebrow{{position:absolute;left:80px;top:50%;transform:translateY(-90px);font-size:34px;letter-spacing:12px;color:#9bd6ff;text-transform:uppercase;font-weight:600;text-shadow:0 2px 8px rgba(0,0,0,0.6)}}
.scene-intro .intro-title{{position:absolute;left:80px;right:80px;top:50%;transform:translateY(-10px);font-size:120px;font-weight:800;line-height:1.04;letter-spacing:-1.5px;text-shadow:0 4px 24px rgba(0,0,0,0.8)}}
.scene-quote .img{{filter:saturate(0.6)}}
.scene-quote .quote-mark{{position:absolute;left:120px;top:160px;font-size:240px;line-height:1;color:rgba(155,214,255,0.6);font-family:Georgia,serif}}
.scene-quote .quote{{position:absolute;left:160px;right:160px;top:50%;transform:translateY(-50%);font-size:72px;font-weight:300;line-height:1.25;color:#fff;font-style:italic;text-shadow:0 4px 24px rgba(0,0,0,0.6)}}
.scene-quote .quote-source{{position:absolute;left:160px;right:160px;bottom:120px;font-size:30px;letter-spacing:10px;color:#9bd6ff;text-transform:uppercase;font-weight:600}}
.progress-bar{{position:absolute;left:0;right:0;bottom:0;height:6px;background:rgba(255,255,255,0.12)}}
.progress-bar > .fill{{position:absolute;left:0;top:0;bottom:0;width:0;background:linear-gradient(90deg,#6ea8ff,#c08bff,#ffb86b)}}
</style></head><body>
<div id="stage" data-composition-id="{composition_id}" data-start="0" data-duration="{total_secs}" data-track-index="0">
<div id="scene-canvas">
{scene_html}
<div class="progress-bar"><div class="fill"></div></div>
</div>
</div>
<script>
window.__timelines = window.__timelines || {{}};
(function(){{
  var tl = gsap.timeline({{paused:true}});
{tween_js}
  tl.fromTo('.progress-bar > .fill',{{width:'0%'}},{{width:'100%',duration:{total_secs_f},ease:'none'}},0);
  window.__timelines['{composition_id}'] = tl;
}})();
</script>
</body></html>"##
    );

    Ok(html)
}

/// Largest-remainder allocation of `steps` slots across chapters, weighted
/// by audio duration. Floors below 1 per chapter, so chapter coverage is
/// never lost; if `steps < chapters.len()` the effective step count
/// becomes `chapters.len()`.
fn allocate_steps(chapters: &[DbChapter], steps: usize) -> Vec<usize> {
    let n = chapters.len();
    if n == 0 {
        return Vec::new();
    }
    let target = steps.max(n);
    let weights: Vec<u64> = chapters
        .iter()
        .map(|c| (c.duration_ms.unwrap_or(0).max(0) as u64).max(1))
        .collect();
    let total: u64 = weights.iter().sum();
    let raw: Vec<f64> = weights
        .iter()
        .map(|w| (*w as f64) * (target as f64) / (total as f64))
        .collect();
    let mut alloc: Vec<usize> = raw.iter().map(|x| x.floor() as usize).collect();
    for a in &mut alloc {
        if *a == 0 {
            *a = 1;
        }
    }
    let mut sum: usize = alloc.iter().sum();
    if sum >= target {
        return alloc;
    }
    let mut frac: Vec<(usize, f64)> = raw
        .iter()
        .enumerate()
        .map(|(i, x)| (i, x - x.floor()))
        .collect();
    frac.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut cursor = 0usize;
    while sum < target && !frac.is_empty() {
        alloc[frac[cursor % frac.len()].0] += 1;
        sum += 1;
        cursor += 1;
    }
    alloc
}

const MAX_IMAGE_INLINE_BYTES: usize = 2 * 1024 * 1024;

fn read_image_as_data_url(path: &Path, max_bytes: usize) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() as usize > max_bytes {
        tracing::warn!(
            path = %path.display(),
            size = metadata.len(),
            max = max_bytes,
            "hyperframes: image too large to inline; falling back to gradient"
        );
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    };
    Some(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn split_into_sentences(s: &str, max: usize) -> Vec<String> {
    if max == 0 || s.trim().is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        buf.push(*c);
        let is_terminator = matches!(*c, '.' | '!' | '?');
        let next_is_space = chars.get(i + 1).map(|n| n.is_whitespace()).unwrap_or(true);
        if is_terminator && next_is_space {
            let t = buf.trim().trim_start_matches('#').trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            buf.clear();
            if out.len() >= max {
                return out;
            }
        }
    }
    let t = buf.trim().trim_start_matches('#').trim().to_string();
    if !t.is_empty() && out.len() < max {
        out.push(t);
    }
    out
}

fn truncate_sentence(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let take: String = trimmed.chars().take(max_chars).collect();
    let cut = take.rfind(' ').unwrap_or(take.len());
    let mut result: String = take.chars().take(cut).collect();
    result.push('…');
    result
}

async fn mux_hyperframes_visual_with_audio(
    state: &AppState,
    chapters: &[DbChapter],
    visual_path: &Path,
    out_path: &Path,
    landscape: bool,
) -> Result<()> {
    let bin = state.config().ffmpeg_bin.trim();
    if bin.is_empty() {
        return Err(Error::Config("ffmpeg_bin is not configured".into()));
    }
    let dir = out_path
        .parent()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("youtube output path has no parent")))?;
    let audio_list_path = out_path.with_extension("hyperframes-audio.txt");
    let mut list = String::new();
    for c in chapters {
        let wav = dir.join(format!("ch-{}.wav", c.number));
        if !wav.exists() {
            return Err(Error::Conflict(format!(
                "audio missing on disk: {}",
                wav.display()
            )));
        }
        let abs = std::fs::canonicalize(&wav).map_err(|e| {
            Error::Other(anyhow::anyhow!("canonicalize audio {}: {e}", wav.display()))
        })?;
        let escaped = abs.display().to_string().replace('\'', r"'\''");
        list.push_str("file '");
        list.push_str(&escaped);
        list.push_str("'\n");
    }
    std::fs::write(&audio_list_path, list)
        .map_err(|e| Error::Other(anyhow::anyhow!("write hyperframes audio concat list: {e}")))?;

    // The Hyperframes service hard-codes its viewport to 1080×1920
    // portrait regardless of what we ask for. For landscape output the
    // composition HTML rotates an inner 1920×1080 canvas inside the
    // portrait frame; we then apply `transpose=1` here to put it
    // upright. Vertical (Shorts) output is already correct so we copy
    // the video stream straight through, matching the original
    // mux behaviour.
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("warning")
        .arg("-stream_loop")
        .arg("-1")
        .arg("-i")
        .arg(visual_path)
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&audio_list_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("1:a:0");
    if landscape {
        cmd.arg("-vf")
            .arg("transpose=1")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryfast")
            .arg("-profile:v")
            .arg("high")
            .arg("-level:v")
            .arg("4.0")
            .arg("-crf")
            .arg("20")
            .arg("-pix_fmt")
            .arg("yuv420p");
    } else {
        cmd.arg("-c:v").arg("copy");
    }
    cmd.arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-shortest")
        .arg("-movflags")
        .arg("+faststart")
        .arg(out_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = cmd
        .output()
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("spawn ffmpeg (hyperframes mux): {e}")))?;
    let _ = std::fs::remove_file(&audio_list_path);
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(Error::Other(anyhow::anyhow!(
            "ffmpeg (hyperframes mux) exited with {}: {}",
            output.status,
            tail.trim_end()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // ffmpeg encoder driver; grouping flags into a struct adds noise without value
async fn encode_mp4_segmented<F, Fut>(
    state: &AppState,
    images: &[ImageSegment],
    wavs: &[PathBuf],
    out_path: &Path,
    total_ms: u64,
    vertical: bool,
    like_subscribe_overlay: bool,
    mut on_progress: F,
) -> Result<()>
where
    F: FnMut(f32) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let bin = state.config().ffmpeg_bin.trim();
    if bin.is_empty() {
        return Err(Error::Config("ffmpeg_bin is not configured".into()));
    }
    if images.is_empty() {
        return Err(Error::Other(anyhow::anyhow!(
            "encode_mp4_segmented: no image segments"
        )));
    }
    if wavs.is_empty() {
        return Err(Error::Other(anyhow::anyhow!(
            "encode_mp4_segmented: no audio inputs"
        )));
    }
    if let Ok(meta) = std::fs::metadata(out_path) {
        if meta.len() > 0 {
            info!(out = ?out_path, bytes = meta.len(), "ffmpeg: reusing existing encode");
            on_progress(1.0).await;
            return Ok(());
        }
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Other(anyhow::anyhow!("mkdir {parent:?}: {e}")))?;
    }

    let parent = out_path.parent().unwrap_or(Path::new("."));

    // Audio concat list — one entry per WAV. Independent of image
    // segments; ffmpeg just needs the total durations to match.
    let audio_concat_path = out_path.with_extension("audio.txt");
    let mut audio_body = String::new();
    for wav in wavs {
        if !wav.exists() {
            return Err(Error::Other(anyhow::anyhow!(
                "chapter wav missing: {wav:?}"
            )));
        }
        let abs = std::fs::canonicalize(wav)
            .map_err(|e| Error::Other(anyhow::anyhow!("canonicalize {wav:?}: {e}")))?;
        let raw = abs
            .to_str()
            .ok_or_else(|| Error::Other(anyhow::anyhow!("non-utf8 wav path: {abs:?}")))?;
        let escaped = raw.replace('\'', "'\\''");
        audio_body.push_str(&format!("file '{escaped}'\n"));
    }
    tokio::fs::write(&audio_concat_path, audio_body)
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("write audio concat: {e}")))?;

    // Resolve each image segment's source to an absolute path, then
    // compose unique sources to 1920×1080 PNGs (cached so repeated
    // sources — e.g. cover-as-fallback for art-less chapters, or
    // chapter cover tile reused as lead-in for many slides — render
    // once).
    let mut composite_cache: std::collections::HashMap<PathBuf, PathBuf> =
        std::collections::HashMap::new();
    let mut per_segment_composite: Vec<PathBuf> = Vec::with_capacity(images.len());
    let mut composites_to_clean: Vec<PathBuf> = Vec::new();
    for seg in images {
        let src_abs = std::fs::canonicalize(&seg.image_src).map_err(|e| {
            Error::Other(anyhow::anyhow!(
                "canonicalize image {:?}: {e}",
                seg.image_src
            ))
        })?;
        if let Some(cached) = composite_cache.get(&src_abs) {
            per_segment_composite.push(cached.clone());
            continue;
        }
        let stem = src_abs.file_stem().and_then(|s| s.to_str()).unwrap_or("bg");
        // Distinct cache filename per orientation so a horizontal +
        // vertical encode of the same source don't clobber each other.
        let suffix = if vertical { "v" } else { "h" };
        let dest = parent.join(format!("youtube-bg-{stem}-{suffix}.png"));
        compose_background(bin, &src_abs, &dest, vertical).await?;
        let dest_abs = std::fs::canonicalize(&dest)
            .map_err(|e| Error::Other(anyhow::anyhow!("canonicalize composite: {e}")))?;
        composite_cache.insert(src_abs, dest_abs.clone());
        composites_to_clean.push(dest.clone());
        per_segment_composite.push(dest_abs);
    }

    info!(
        image_segments = images.len(),
        audio_files = wavs.len(),
        unique_images = composite_cache.len(),
        total_ms,
        out = ?out_path,
        "ffmpeg: starting segmented encode"
    );
    let started = std::time::Instant::now();

    // Build the ffmpeg command:
    //   * one `-loop 1 -framerate 5 -t <secs> -i <png>` per image segment
    //   * audio concat input
    //   * `concat=n=N:v=1:a=0` filter to splice the video segments
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error");

    for (seg, composite) in images.iter().zip(per_segment_composite.iter()) {
        let dur_secs = seg.duration_ms as f64 / 1000.0;
        cmd.arg("-loop").arg("1");
        cmd.arg("-framerate").arg("5");
        cmd.arg("-t").arg(format!("{dur_secs:.3}"));
        cmd.arg("-i").arg(composite);
    }
    let audio_input_index = images.len();
    cmd.arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&audio_concat_path);

    // [0:v][1:v]…[N-1:v]concat=n=N:v=1:a=0[v]. Single-segment encodes
    // skip the filter and map [0:v] directly — saves a filter graph
    // setup for the common single-image case (translations, art-less
    // chapters). When the like-and-subscribe overlay is enabled we
    // always force a filter graph so we can chain the drawtext on the
    // end.
    let overlay_clause = like_subscribe_overlay
        .then(|| build_like_subscribe_drawtext(total_ms))
        .flatten();
    match (images.len(), overlay_clause.as_deref()) {
        // Single image, no overlay: cheapest path — direct map, no filter.
        (1, None) => {
            cmd.arg("-map").arg("0:v");
        }
        // Single image, with overlay: one drawtext filter.
        (1, Some(draw)) => {
            cmd.arg("-filter_complex").arg(format!("[0:v]{draw}[v]"));
            cmd.arg("-map").arg("[v]");
        }
        // Multi-segment, no overlay: concat directly into [v].
        (_, None) => {
            let mut filter = String::new();
            for i in 0..images.len() {
                filter.push_str(&format!("[{i}:v]"));
            }
            filter.push_str(&format!("concat=n={}:v=1:a=0[v]", images.len()));
            cmd.arg("-filter_complex").arg(&filter);
            cmd.arg("-map").arg("[v]");
        }
        // Multi-segment, with overlay: concat → drawtext.
        (_, Some(draw)) => {
            let mut filter = String::new();
            for i in 0..images.len() {
                filter.push_str(&format!("[{i}:v]"));
            }
            filter.push_str(&format!("concat=n={}:v=1:a=0[vc];", images.len()));
            filter.push_str(&format!("[vc]{draw}[v]"));
            cmd.arg("-filter_complex").arg(&filter);
            cmd.arg("-map").arg("[v]");
        }
    }
    cmd.arg("-map").arg(format!("{audio_input_index}:a"));

    cmd.arg("-c:v")
        .arg("libx264")
        .arg("-tune")
        .arg("stillimage")
        .arg("-preset")
        .arg("veryfast")
        .arg("-profile:v")
        .arg("high")
        .arg("-level:v")
        .arg("4.0")
        .arg("-crf")
        .arg("22")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-r")
        .arg("5")
        .arg("-g")
        .arg("300")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-ar")
        .arg("48000")
        .arg("-shortest")
        .arg("-movflags")
        .arg("+faststart")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-nostats")
        .arg(out_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Other(anyhow::anyhow!("spawn ffmpeg `{bin}`: {e}")))?;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Drain stderr concurrently with stdout — same deadlock-avoidance
    // logic as the simpler encoder path.
    let stderr_handle = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
        if let Some(mut e) = stderr {
            let mut chunk = [0u8; 4096];
            loop {
                match e.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        let limit = 8 * 1024;
                        if buf.len() > limit {
                            let start = buf.len() - limit;
                            buf.drain(..start);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    if let Some(stdout) = stdout {
        let mut reader = BufReader::new(stdout).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    if let Some(us) = line.strip_prefix("out_time_us=") {
                        if total_ms > 0 {
                            if let Ok(us) = us.trim().parse::<i64>() {
                                let pct = (us.max(0) as f32) / (total_ms as f32 * 1000.0);
                                on_progress(pct.clamp(0.0, 1.0)).await;
                            }
                        }
                    } else if line == "progress=end" {
                        on_progress(1.0).await;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    }

    let stderr_tail = stderr_handle.await.unwrap_or_default();
    let status = child
        .wait()
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("await ffmpeg: {e}")))?;
    if !status.success() {
        return Err(Error::Other(anyhow::anyhow!(
            "ffmpeg exited with {status}: {}",
            stderr_tail
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }

    let bytes = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
    info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        out_bytes = bytes,
        "ffmpeg: segmented encode complete"
    );

    // Best-effort cleanup of intermediates.
    let _ = tokio::fs::remove_file(&audio_concat_path).await;
    for p in composites_to_clean {
        let _ = tokio::fs::remove_file(&p).await;
    }
    Ok(())
}

/// Build the `drawtext=…` clause for the "Like & Subscribe!" overlay,
/// or `None` if the video is too short to host it cleanly. Pure string
/// builder — no I/O. Two visibility windows when the runtime is long
/// enough (early: 5–13 s, late: last 10 s); a single early window for
/// shorter clips so the call-to-action still appears once.
///
/// We resolve the font via fontconfig (`font=Sans-Bold`) rather than a
/// hard-coded `fontfile` path so this works across distros without a
/// new config option. Distros with an ffmpeg built without freetype +
/// fontconfig will surface the failure at encode time; the user can
/// then disable the toggle.
fn build_like_subscribe_drawtext(total_ms: u64) -> Option<String> {
    // Anything shorter than ~6 s would either flash and disappear or
    // collide with the start. Skip rather than produce a janky result.
    if total_ms < 6_000 {
        return None;
    }
    let total_s = (total_ms as f64) / 1000.0;
    // Late-window threshold: last 10 s of the video, but never closer
    // than 5 s after the early window so the two don't overlap.
    let early_start = 5.0_f64;
    let early_end = (early_start + 8.0).min(total_s - 0.5);
    let want_late = total_s >= 25.0;
    let enable_expr = if want_late {
        let late_start = (total_s - 10.0).max(early_end + 5.0);
        format!(
            "between(t\\,{e0:.2}\\,{e1:.2})+gte(t\\,{ls:.2})",
            e0 = early_start,
            e1 = early_end,
            ls = late_start,
        )
    } else {
        format!(
            "between(t\\,{e0:.2}\\,{e1:.2})",
            e0 = early_start,
            e1 = early_end,
        )
    };

    // Inside `-filter_complex`, drawtext value separators are `:` and
    // each value is wrapped in single quotes. The `&` and `,` inside
    // `enable=` are escaped with `\\` because they're filter-graph
    // metacharacters — the explicit escapes above already handle the
    // commas inside `between()`.
    Some(format!(
        "drawtext=font=Sans-Bold:text='LIKE \\& SUBSCRIBE!'\
         :fontsize=h*0.06:fontcolor=white\
         :box=1:boxcolor=black@0.6:boxborderw=24\
         :x=(w-text_w)/2:y=h-text_h-h*0.10\
         :enable='{enable_expr}'"
    ))
}

/// Render a single background frame composited from `src`: the source
/// image scaled + cropped + blurred as a backdrop, with the same image
/// scaled crisply on top in the centre. Result is a static PNG that the
/// encoder loops cheaply.
///
/// When `vertical` is true we produce a 1080×1920 frame for YouTube
/// Shorts (the inset is 1080×1080 — the source is square). Otherwise
/// the layout is the legacy 1920×1080 widescreen.
async fn compose_background(bin: &str, src: &Path, dest: &Path, vertical: bool) -> Result<()> {
    let composite_filter = if vertical {
        "[0:v]split=2[b][f];\
        [b]scale=1080:1920:force_original_aspect_ratio=increase,\
            crop=1080:1920,boxblur=20:20,eq=brightness=-0.15[bg];\
        [f]scale=1080:1080:force_original_aspect_ratio=decrease,\
            setsar=1[fg];\
        [bg][fg]overlay=x=(W-w)/2:y=(H-h)/2"
    } else {
        "[0:v]split=2[b][f];\
        [b]scale=1920:1080:force_original_aspect_ratio=increase,\
            crop=1920:1080,boxblur=20:20,eq=brightness=-0.15[bg];\
        [f]scale=1080:1080:force_original_aspect_ratio=decrease,\
            setsar=1[fg];\
        [bg][fg]overlay=x=(W-w)/2:y=(H-h)/2"
    };
    let started = std::time::Instant::now();
    let status = tokio::process::Command::new(bin)
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(src)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(composite_filter)
        .arg(dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| Error::Other(anyhow::anyhow!("spawn ffmpeg compose: {e}")))?;
    if !status.success() {
        return Err(Error::Other(anyhow::anyhow!(
            "ffmpeg compose exited with {status}"
        )));
    }
    info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        src = ?src,
        out = ?dest,
        "ffmpeg: composite background ready"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Upload helper (shared between modes)
// ---------------------------------------------------------------------------

/// Open a resumable upload session and stream `mp4_path` to YouTube,
/// reporting progress to the websocket on `[span_start..span_end]`.
async fn upload_one(
    ctx: &JobContext,
    job: &JobRow,
    access_token: &str,
    mp4_path: &Path,
    metadata: &upload::VideoMetadata,
    span_start: f32,
    span_end: f32,
) -> Result<upload::UploadResult> {
    let total_bytes = tokio::fs::metadata(mp4_path)
        .await
        .map(|m| m.len())
        .map_err(|e| Error::Other(anyhow::anyhow!("stat mp4 {mp4_path:?}: {e}")))?;
    info!(
        mp4 = ?mp4_path,
        mp4_bytes = total_bytes,
        "publish_youtube: opening upload session"
    );
    let upload_url = upload::start_session(access_token, metadata, total_bytes).await?;

    ctx.progress(job, "uploading", span_start.clamp(0.0, 0.99))
        .await;

    let job_for_progress = job.clone();
    let ctx_for_progress = ctx.clone();
    let span = span_end - span_start;
    upload::upload_file(&upload_url, mp4_path, move |sent, total| {
        let job = job_for_progress.clone();
        let ctx = ctx_for_progress.clone();
        async move {
            let frac = if total == 0 {
                0.0
            } else {
                sent as f32 / total as f32
            };
            let overall = span_start + (frac * span);
            ctx.progress(&job, "uploading", overall.clamp(0.0, 0.99))
                .await;
        }
    })
    .await
}

// ---------------------------------------------------------------------------
// YouTube metadata building
// ---------------------------------------------------------------------------

fn build_book_metadata(
    book: &DbAudiobook,
    chapters: &[DbChapter],
    language: &str,
    privacy: &str,
    description_override: Option<&str>,
    footer: Option<&str>,
) -> upload::VideoMetadata {
    let is_short = book.is_short.unwrap_or(false);
    // YouTube Shorts must have `#Shorts` somewhere in the title or
    // description — the platform treats it as the opt-in signal for
    // vertical playback. Prefer the title so it's visible at a glance,
    // trimming the book title first to leave room.
    let safe_title = sanitize_for_youtube(&book.title);
    let title = if is_short {
        let head = trim_to(&safe_title, 100 - " #Shorts".len());
        format!("{head} #Shorts")
    } else {
        trim_to(&safe_title, 100)
    };

    let raw_desc = match description_override {
        Some(s) => s.to_string(),
        None => render_description(book, chapters, language),
    };
    let with_footer = append_footer(&raw_desc, footer);
    // Belt-and-braces: also drop the hashtag at the end of the
    // description in case YouTube's Shorts heuristic looks there.
    // Sanitize last so any `<…>` run from prose, footer text, or
    // override input gets stripped before we ship to YouTube.
    let description = trim_to(
        &sanitize_for_youtube(&if is_short {
            format!("{}\n\n#Shorts", with_footer.trim_end())
        } else {
            with_footer
        }),
        5000,
    );

    let mut tags: Vec<String> = Vec::new();
    if let Some(g) = book.genre.as_deref().filter(|g| !g.trim().is_empty()) {
        tags.push(g.to_string());
    }
    tags.push(language.to_string());
    tags.push("audiobook".into());
    tags.push("AidBooks".into());
    if is_short {
        tags.push("Shorts".into());
    }

    upload::VideoMetadata {
        snippet: upload::Snippet {
            title,
            description,
            tags,
            // 22 = "People & Blogs" — safer default than 27 (Education) for
            // AI-narrated content per the design doc.
            category_id: "22".to_string(),
            default_language: Some(language.to_string()),
            default_audio_language: Some(language.to_string()),
        },
        status: upload::VideoStatus {
            privacy_status: privacy.to_string(),
            // Never auto-flag as kids content; leave that opt-in.
            self_declared_made_for_kids: false,
        },
    }
}

fn build_chapter_metadata(
    book: &DbAudiobook,
    chapter: &DbChapter,
    total_chapters: u32,
    language: &str,
    privacy: &str,
    footer: Option<&str>,
) -> upload::VideoMetadata {
    // YouTube caps titles at 100 chars; keep the chapter title prominent so
    // the playlist scans well in the YouTube UI. Sanitize before
    // trimming so a stripped `<…>` doesn't leave us under-budget.
    let raw_title = format!(
        "{} — Ch. {}: {}",
        book.title.trim(),
        chapter.number,
        chapter.title.trim()
    );
    let title = trim_to(&sanitize_for_youtube(&raw_title), 100);

    let labels = crate::i18n::description_labels(language);

    let mut desc = String::new();
    desc.push_str(&(labels.from_book)(book.title.trim()));
    desc.push_str("\n\n");
    if let Some(s) = chapter.synopsis.as_deref().filter(|s| !s.trim().is_empty()) {
        desc.push_str(s.trim());
        desc.push_str("\n\n");
    }
    desc.push_str(&(labels.chapter_of)(chapter.number as u32, total_chapters));
    desc.push_str(".\n\n");
    desc.push_str(labels.generated_with);
    desc.push('\n');
    let description = trim_to(&sanitize_for_youtube(&append_footer(&desc, footer)), 5000);

    let mut tags: Vec<String> = Vec::new();
    if let Some(g) = book.genre.as_deref().filter(|g| !g.trim().is_empty()) {
        tags.push(g.to_string());
    }
    tags.push(language.to_string());
    tags.push("audiobook".into());
    tags.push("AidBooks".into());

    upload::VideoMetadata {
        snippet: upload::Snippet {
            title,
            description,
            tags,
            category_id: "22".to_string(),
            default_language: Some(language.to_string()),
            default_audio_language: Some(language.to_string()),
        },
        status: upload::VideoStatus {
            privacy_status: privacy.to_string(),
            self_declared_made_for_kids: false,
        },
    }
}

fn render_description(book: &DbAudiobook, chapters: &[DbChapter], language: &str) -> String {
    let labels = crate::i18n::description_labels(language);
    let mut s = String::new();

    // Topic is the user's prompt. It's only persisted in the book's
    // primary language — including it on a translated upload would mix
    // languages, so we skip it when publishing a translation.
    let primary = book.language.as_deref().unwrap_or("en");
    if primary == language && !book.topic.trim().is_empty() {
        s.push_str(book.topic.trim());
        s.push_str("\n\n");
    }

    if let Some(g) = book.genre.as_deref().filter(|g| !g.trim().is_empty()) {
        s.push_str(labels.genre_label);
        s.push(' ');
        s.push_str(g);
        s.push_str("\n\n");
    }

    // Lead with the translated chapter synopses — that's the actual
    // book text in the publish language and the most useful context
    // a YouTube viewer can scan before pressing play.
    for ch in chapters {
        if let Some(syn) = ch
            .synopsis
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            s.push_str(syn);
            s.push_str("\n\n");
        }
    }

    // Chapter listing with timestamps. Titles are already loaded in
    // the publish language by `load_chapters(audiobook, language)`.
    s.push_str(labels.chapters_heading);
    s.push('\n');
    let mut running_ms: u64 = 0;
    for ch in chapters {
        s.push_str(&format!(
            "{} — {}\n",
            format_timestamp(running_ms),
            ch.title.trim()
        ));
        running_ms = running_ms.saturating_add(ch.duration_ms.unwrap_or(0).max(0) as u64);
    }

    s.push('\n');
    s.push_str(labels.generated_with);
    s.push('\n');
    s
}

fn render_playlist_description(
    book: &DbAudiobook,
    override_text: Option<&str>,
    footer: Option<&str>,
    language: &str,
) -> String {
    if let Some(s) = override_text.map(str::trim).filter(|s| !s.is_empty()) {
        return append_footer(s, footer);
    }
    let labels = crate::i18n::description_labels(language);
    let mut s = String::new();
    let primary = book.language.as_deref().unwrap_or("en");
    if primary == language && !book.topic.trim().is_empty() {
        s.push_str(book.topic.trim());
        s.push_str("\n\n");
    }
    if let Some(g) = book.genre.as_deref().filter(|g| !g.trim().is_empty()) {
        s.push_str(labels.genre_label);
        s.push(' ');
        s.push_str(g);
        s.push_str("\n\n");
    }
    s.push_str(labels.generated_with);
    s.push('\n');
    append_footer(&s, footer)
}

/// Appends the per-language admin footer to a description, separated by a
/// blank line. Whitespace-only or `None` footers pass through unchanged so
/// the helper is a no-op when the admin hasn't configured one.
/// Strip X.ai speech tags and any other angle-bracketed runs so the
/// resulting string is safe to send as a YouTube `snippet.title` /
/// `snippet.description`. YouTube rejects either field with HTTP 400
/// `invalidDescription` / `invalidTitle` whenever it contains `<` or
/// `>` (regardless of context — there's no escaping). Songbook prose
/// embeds X.ai tags like `<singing>line</singing>` to drive the TTS,
/// and the description builder pulls chapter synopses verbatim, so
/// the brackets leak through unless we strip them here.
///
/// Behaviour: drops the entire `<…>` run including inner content. For
/// X.ai tags that means `<singing>foo</singing>` becomes `foo` only
/// because the inner content sits *between* the open + close tags,
/// outside the brackets — i.e. the run we strip is `<singing>` and
/// `</singing>` separately. Stray `<` or `>` (no matching partner)
/// is also removed so YouTube can never see one. We don't try to
/// preserve content from malformed input — the worst case is an
/// over-aggressive strip, never a leaked bracket.
fn sanitize_for_youtube(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match (c, in_tag) {
            ('<', _) => in_tag = true,
            ('>', _) => in_tag = false,
            (_, true) => {}
            (c, false) => out.push(c),
        }
    }
    out
}

fn append_footer(body: &str, footer: Option<&str>) -> String {
    let trimmed = footer.map(str::trim).filter(|s| !s.is_empty());
    let Some(f) = trimmed else {
        return body.to_string();
    };
    let mut out = body.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(f);
    out
}

/// Loads the admin-configured description footer for `language` from
/// `youtube_description_footer:<language>`. Falls back to `None` on any
/// DB error or missing row — the publisher carries on without it rather
/// than failing the upload.
async fn load_description_footer(state: &AppState, language: &str) -> Option<String> {
    let trimmed = language.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Row {
        text: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT text FROM youtube_description_footer:`{trimmed}`"
        ))
        .await
        .ok()?
        .take(0)
        .ok()?;
    rows.into_iter()
        .next()
        .map(|r| r.text)
        .filter(|s| !s.trim().is_empty())
}

/// Builds a compact "Models used" block from `generation_event`, or
/// `None` when the audiobook hasn't accumulated any successful events
/// (e.g. a fully mocked book in dev). Lines are bucketed by activity —
/// text / cover / illustrations / narration / animation — and only
/// emitted when at least one model contributed to that bucket. Distinct
/// model names within a bucket are de-duplicated and joined with ", ".
async fn load_credits_block(state: &AppState, audiobook_id: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Row {
        role: String,
        #[serde(default)]
        llm: Option<surrealdb::sql::Thing>,
        // TTS rows stash `voice=<id> duration_ms=… chars=…` here; for
        // narration we read this rather than the placeholder llm link.
        #[serde(default)]
        error: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT role, llm, error FROM generation_event \
             WHERE audiobook = audiobook:`{audiobook_id}` AND success = true"
        ))
        .await
        .ok()?
        .take(0)
        .ok()?;
    if rows.is_empty() {
        return None;
    }

    // Look up display names for the unique non-TTS llm ids in one shot.
    use std::collections::{BTreeMap, BTreeSet};
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for r in &rows {
        if r.role == "tts" {
            continue;
        }
        if let Some(t) = &r.llm {
            let raw = t.id.to_raw();
            if raw != "_default_" {
                wanted.insert(raw);
            }
        }
    }
    let mut name_by_id: BTreeMap<String, String> = BTreeMap::new();
    if !wanted.is_empty() {
        #[derive(Deserialize)]
        struct Meta {
            id: surrealdb::sql::Thing,
            name: String,
        }
        let ids: Vec<String> = wanted.iter().cloned().collect();
        let metas: Vec<Meta> = state
            .db()
            .inner()
            .query(
                "SELECT id, name FROM llm \
                 WHERE record::id(id) INSIDE $ids",
            )
            .bind(("ids", ids))
            .await
            .ok()?
            .take(0)
            .ok()?;
        for m in metas {
            name_by_id.insert(m.id.id.to_raw(), m.name);
        }
    }

    // Bucket the unique model names per category. We use BTreeSet to keep
    // ordering stable across renders so reuploads don't churn the
    // description for cosmetic reasons.
    let mut text: BTreeSet<String> = BTreeSet::new();
    let mut cover: BTreeSet<String> = BTreeSet::new();
    let mut illust: BTreeSet<String> = BTreeSet::new();
    let mut narr: BTreeSet<String> = BTreeSet::new();
    let mut anim: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        let label_for_llm = || -> Option<String> {
            let id = r.llm.as_ref()?.id.to_raw();
            if id == "_default_" {
                return None;
            }
            name_by_id.get(&id).cloned().or(Some(id))
        };
        match r.role.as_str() {
            "outline" | "chapter" | "title" | "translate" => {
                if let Some(n) = label_for_llm() {
                    text.insert(n);
                }
            }
            "cover" => {
                if let Some(n) = label_for_llm() {
                    cover.insert(n);
                }
            }
            "paragraph_image" => {
                if let Some(n) = label_for_llm() {
                    illust.insert(n);
                }
            }
            "manim_code" | "paragraph_visual" => {
                if let Some(n) = label_for_llm() {
                    anim.insert(n);
                }
            }
            "tts" => {
                // Voice id parsed from `voice=<id> …` is the most useful
                // label here — the stored llm link is a placeholder.
                if let Some(voice) = r
                    .error
                    .as_deref()
                    .and_then(|s| s.split_whitespace().find_map(|t| t.strip_prefix("voice=")))
                {
                    narr.insert(voice.to_string());
                }
            }
            _ => {}
        }
    }

    let mut lines: Vec<String> = Vec::new();
    let push_bucket = |lines: &mut Vec<String>, label: &str, set: &BTreeSet<String>| {
        if set.is_empty() {
            return;
        }
        let joined = set.iter().cloned().collect::<Vec<_>>().join(", ");
        lines.push(format!("• {label}: {joined}"));
    };
    push_bucket(&mut lines, "Text", &text);
    push_bucket(&mut lines, "Cover", &cover);
    push_bucket(&mut lines, "Illustrations", &illust);
    push_bucket(&mut lines, "Narration", &narr);
    push_bucket(&mut lines, "Animation", &anim);
    if lines.is_empty() {
        return None;
    }
    let mut out = String::from("Models used:\n");
    out.push_str(&lines.join("\n"));
    Some(out)
}

/// Splices an optional credits block into the existing per-language
/// admin footer so the rest of the publisher's plumbing stays unchanged.
/// The two are joined by a blank line; either side may be absent.
fn combine_credits_and_footer(credits: Option<&str>, footer: Option<&str>) -> Option<String> {
    let c = credits.map(str::trim).filter(|s| !s.is_empty());
    let f = footer.map(str::trim).filter(|s| !s.is_empty());
    match (c, f) {
        (None, None) => None,
        (Some(c), None) => Some(c.to_string()),
        (None, Some(f)) => Some(f.to_string()),
        (Some(c), Some(f)) => Some(format!("{c}\n\n{f}")),
    }
}

fn format_timestamp(ms: u64) -> String {
    let total_secs = ms / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn trim_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbAudiobook {
    title: String,
    topic: String,
    #[serde(default)]
    genre: Option<String>,
    /// BCP-47 of the language the book was originally generated in.
    /// `topic` and `title` live in this language; chapter rows in any
    /// other language are translations. The description builder uses
    /// this to decide whether the user-supplied topic is safe to
    /// include verbatim (publish language matches primary) or has to
    /// be skipped (different language → topic would clash).
    #[serde(default)]
    language: Option<String>,
    /// `true` for YouTube Shorts: vertical 1080×1920 encode and the
    /// `#Shorts` hashtag appended to the description so the platform
    /// classifies the upload correctly.
    #[serde(default)]
    is_short: Option<bool>,
    /// Number of song snippets the audiobook was created with. The
    /// publisher reads `<storage>/<audiobook>/snippets/snippet-<i>.wav`
    /// for `i = 1..=snippet_count` and intercalates them between
    /// chapters. Missing files are tolerated (publisher just skips
    /// them); zero / absent value disables splicing entirely.
    #[serde(default)]
    snippet_count: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DbChapter {
    number: i64,
    title: String,
    status: String,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    chapter_art_path: Option<String>,
    /// Markdown prose narrated by TTS. Used both for the playlist-mode
    /// description fallback and for SRT subtitle generation.
    #[serde(default)]
    body_md: Option<String>,
    /// Paragraph illustrations populated by the `chapter_paragraphs`
    /// orchestrator. Anchored to the primary language — translations
    /// have an empty array and fall back to the single chapter art tile.
    #[serde(default)]
    paragraphs: Option<Vec<DbParagraph>>,
}

#[derive(Debug, Deserialize, Clone)]
struct DbParagraph {
    /// The encoder walks paragraphs in array order, so the explicit
    /// index isn't read here. Kept on the struct because it's useful
    /// in debug logs / future diagnostics.
    #[serde(default)]
    #[allow(dead_code)]
    index: i64,
    #[serde(default)]
    char_count: Option<i64>,
    /// Empty / `None` for non-visual paragraphs the LLM extract pass
    /// skipped — those never get tile jobs and the encoder ignores them.
    /// Loaded but not read at the moment: the slideshow builder gates
    /// inclusion on `image_paths` non-empty + on-disk existence rather
    /// than on this metadata, so older tiles generated before the
    /// extract field landed still get displayed.
    #[serde(default)]
    #[allow(dead_code)]
    scene_description: Option<String>,
    #[serde(default)]
    image_paths: Vec<String>,
}

struct PublicationLookup {
    id: String,
    privacy_status: String,
    mode: String,
    playlist_id: Option<String>,
    review: bool,
    /// Per-publication override for the like-and-subscribe overlay.
    /// `None` = inherit the global setting; `Some(_)` = explicit
    /// override that wins regardless of the singleton.
    like_subscribe_overlay: Option<bool>,
}

async fn find_publication(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
) -> Result<Option<PublicationLookup>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        id: surrealdb::sql::Thing,
        privacy_status: String,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        playlist_id: Option<String>,
        #[serde(default)]
        review: Option<bool>,
        #[serde(default)]
        like_subscribe_overlay: Option<bool>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, privacy_status, mode, playlist_id, review, \
                    like_subscribe_overlay \
             FROM youtube_publication \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang LIMIT 1"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt pub fetch: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt pub fetch (decode): {e}")))?;
    Ok(rows.into_iter().next().map(|r| PublicationLookup {
        id: r.id.id.to_raw(),
        privacy_status: r.privacy_status,
        mode: r.mode.unwrap_or_else(|| "single".to_string()),
        playlist_id: r.playlist_id,
        review: r.review.unwrap_or(false),
        like_subscribe_overlay: r.like_subscribe_overlay,
    }))
}

/// Resolve the YouTube playlist this audiobook should publish into via
/// its podcast assignment. Returns:
///   * `Ok(Some(id))` — the audiobook is in a podcast that has a synced
///     YouTube playlist.
///   * `Ok(None)` — the audiobook isn't in a podcast, or its podcast
///     hasn't been synced to YouTube yet.
///
/// Failures are non-fatal for the publish path; callers fall back to the
/// per-publication playlist behaviour on `None`.
async fn load_podcast_playlist(state: &AppState, audiobook_id: &str) -> Result<Option<String>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        #[serde(default)]
        youtube_playlist_id: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT youtube_playlist_id FROM podcast \
             WHERE id IN (SELECT VALUE podcast FROM audiobook:`{audiobook_id}` \
                          WHERE podcast != NONE) LIMIT 1"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt podcast playlist: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt podcast playlist (decode): {e}")))?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.youtube_playlist_id)
        .filter(|s| !s.trim().is_empty()))
}

async fn load_audiobook(state: &AppState, id: &str) -> Result<DbAudiobook> {
    let rows: Vec<DbAudiobook> = state
        .db()
        .inner()
        .query(format!(
            "SELECT title, topic, genre, language, is_short, snippet_count FROM audiobook:`{id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt load book: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt load book (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{id}"),
    })
}

async fn load_chapters(
    state: &AppState,
    audiobook_id: &str,
    language: &str,
) -> Result<Vec<DbChapter>> {
    let rows: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT number, title, status, duration_ms, synopsis, chapter_art_path, body_md, paragraphs \
             FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` AND language = $lang \
             ORDER BY number ASC"
        ))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt load chapters: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt load chapters (decode): {e}")))?;
    Ok(rows)
}

async fn resolve_access_token(state: &AppState, user: &UserId) -> Result<String> {
    // Delegate to the shared helper so the publish path benefits from
    // refresh-token rotation persistence and the auto-drop-on-
    // invalid_grant behaviour. `None` (no row) maps to Unauthorized so
    // the publish job surfaces the same "please reconnect" outcome it
    // did before.
    match account::access_token(state, user).await? {
        Some(t) => Ok(t),
        None => Err(Error::Unauthorized),
    }
}

/// Best-effort: attempt to flip a playlist's `podcastStatus` to
/// `enabled`. Called from the publish job after the first video has
/// successfully been added to the podcast's playlist. We swallow all
/// errors here — the video is already up; the user can re-trigger via
/// the manual `Sync to YouTube` button if YouTube still rejects this.
async fn try_designate_podcast(
    state: &AppState,
    access_token: &str,
    playlist_id: &str,
    book_title: &str,
    language: &str,
) {
    // Re-`PUT` the playlist with `podcastStatus = enabled`. We need a
    // title to send (YouTube rejects partial PUTs); the audiobook's
    // title is a serviceable placeholder when we don't have the
    // podcast's row at hand. The handler-side sync flow always rewrites
    // these fields with the true podcast title + description on the
    // user's next save, so transient drift is fine.
    let title = trim_to(book_title, 150);
    match playlist::update_playlist(
        access_token,
        playlist_id,
        &title,
        "",
        // Publish-time privacy comes from the publication; the playlist
        // designation just needs *some* valid value here. Public
        // matches handlers/podcasts.rs::PODCAST_PLAYLIST_PRIVACY.
        "public",
        Some(language),
        true,
    )
    .await
    {
        Ok(()) => {
            tracing::info!(playlist_id, "publish_youtube: podcast designation enabled");
        }
        Err(Error::Conflict(msg)) => {
            // YouTube still considers the playlist ineligible (e.g.
            // channel not allowed to host podcasts). Log + move on.
            tracing::warn!(
                playlist_id,
                error = %msg,
                "publish_youtube: podcast designation declined"
            );
        }
        Err(Error::Unauthorized) => {
            tracing::warn!(
                playlist_id,
                "publish_youtube: podcast designation unauthorized"
            );
        }
        Err(e) => {
            tracing::warn!(
                playlist_id,
                error = %e,
                "publish_youtube: podcast designation failed"
            );
        }
    }
    // Touch state so the unused-import lint stays quiet on builds where
    // the helper is the sole consumer of this module path. Cheap noop.
    let _ = state;
}

async fn drop_account(state: &AppState, user: &UserId) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "DELETE youtube_account WHERE owner = user:`{}`",
            user.0
        ))
        .await
        .map_err(|e| Error::Database(format!("yt drop account: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt drop account: {e}")))?;
    Ok(())
}

async fn mark_published(
    state: &AppState,
    publication_id: &str,
    video_id: &str,
    video_url: &str,
) -> Result<()> {
    // Use SurrealQL `time::now()` for datetime fields. Binding a
    // chrono::DateTime via serde produces an RFC3339 *string*, which
    // SurrealDB v2 rejects against an `option<datetime>` column.
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                video_id = $vid, \
                video_url = $vurl, \
                last_error = NONE, \
                published_at = time::now(), \
                updated_at = time::now()"
        ))
        .bind(("vid", video_id.to_string()))
        .bind(("vurl", video_url.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt mark published: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt mark published: {e}")))?;
    Ok(())
}

async fn mark_playlist_created(
    state: &AppState,
    publication_id: &str,
    playlist_id: &str,
    playlist_url: &str,
) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                playlist_id = $pid, \
                playlist_url = $purl, \
                last_error = NONE, \
                updated_at = time::now()"
        ))
        .bind(("pid", playlist_id.to_string()))
        .bind(("purl", playlist_url.to_string()))
        .await
        .map_err(|e| Error::Database(format!("yt mark playlist: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt mark playlist: {e}")))?;
    Ok(())
}

async fn mark_preview_ready(state: &AppState, publication_id: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                preview_ready_at = time::now(), \
                last_error = NONE, \
                updated_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt mark preview_ready: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt mark preview_ready: {e}")))?;
    Ok(())
}

async fn mark_playlist_complete(state: &AppState, publication_id: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                last_error = NONE, \
                published_at = time::now(), \
                updated_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt mark playlist done: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt mark playlist done: {e}")))?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct PublicationVideoRow {
    chapter_number: i64,
    #[serde(default)]
    video_id: Option<String>,
}

async fn load_publication_videos(
    state: &AppState,
    publication_id: &str,
) -> Result<Vec<PublicationVideoRow>> {
    let rows: Vec<PublicationVideoRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT chapter_number, video_id FROM youtube_publication_video \
             WHERE publication = youtube_publication:`{publication_id}` \
             ORDER BY chapter_number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt pub videos load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt pub videos load (decode): {e}")))?;
    Ok(rows)
}

async fn upsert_chapter_video_pending(
    state: &AppState,
    publication_id: &str,
    chapter: &DbChapter,
) -> Result<()> {
    // Delete-then-create keeps the row id deterministic per (pub, chapter)
    // so we don't accumulate multiple rows on retry. The unique index on
    // (publication, chapter_number) would also catch this but DELETE is
    // simpler and idempotent.
    let title = trim_to(&chapter.title, 200);
    state
        .db()
        .inner()
        .query(format!(
            "DELETE youtube_publication_video \
                WHERE publication = youtube_publication:`{publication_id}` \
                  AND chapter_number = $n; \
             CREATE youtube_publication_video CONTENT {{ \
                publication: youtube_publication:`{publication_id}`, \
                chapter_number: $n, \
                title: $t \
             }}"
        ))
        .bind(("n", chapter.number))
        .bind(("t", title))
        .await
        .map_err(|e| Error::Database(format!("yt pub video upsert: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt pub video upsert: {e}")))?;
    Ok(())
}

async fn mark_chapter_video_published(
    state: &AppState,
    publication_id: &str,
    chapter: &DbChapter,
    video_id: &str,
    video_url: &str,
) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication_video SET \
                video_id = $vid, \
                video_url = $vurl, \
                last_error = NONE, \
                published_at = time::now(), \
                updated_at = time::now() \
             WHERE publication = youtube_publication:`{publication_id}` \
               AND chapter_number = $n"
        ))
        .bind(("vid", video_id.to_string()))
        .bind(("vurl", video_url.to_string()))
        .bind(("n", chapter.number))
        .await
        .map_err(|e| Error::Database(format!("yt pub video mark: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("yt pub video mark: {e}")))?;
    Ok(())
}

async fn mark_chapter_video_error(
    state: &AppState,
    publication_id: &str,
    chapter: &DbChapter,
    msg: &str,
) {
    let trimmed = if msg.chars().count() > 500 {
        msg.chars().take(500).collect::<String>()
    } else {
        msg.to_string()
    };
    if let Err(e) = state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication_video SET \
                last_error = $e, updated_at = time::now() \
             WHERE publication = youtube_publication:`{publication_id}` \
               AND chapter_number = $n"
        ))
        .bind(("e", trimmed))
        .bind(("n", chapter.number))
        .await
    {
        warn!(error = %e, publication_id, chapter = chapter.number, "yt pub video error write failed");
    }
}

async fn mark_publication_error(state: &AppState, publication_id: &str, msg: &str) {
    let trimmed = if msg.chars().count() > 500 {
        msg.chars().take(500).collect::<String>()
    } else {
        msg.to_string()
    };
    if let Err(e) = state
        .db()
        .inner()
        .query(format!(
            "UPDATE youtube_publication:`{publication_id}` SET \
                last_error = $e, updated_at = time::now()"
        ))
        .bind(("e", trimmed))
        .await
    {
        warn!(error = %e, publication_id, "yt publication error write failed");
    }
}

/// Convert a fatal error into a `JobOutcome::Fatal` after writing it to the
/// publication row so the UI can surface it.
async fn fail(state: &AppState, publication_id: &str, e: Error) -> JobOutcome {
    let msg = e.to_string();
    mark_publication_error(state, publication_id, &msg).await;
    JobOutcome::Fatal(msg)
}

/// Build + upload one caption track per available language.
///
/// All tracks share the *primary* language's chapter durations — that's
/// the audio playing in the video, so the foreign-language text has to
/// follow that timeline. Within each chapter, the foreign text is
/// distributed proportionally to character count, same algorithm as the
/// primary track.
///
/// Tracks are independent of the video: a failed upload for one language
/// logs and moves on to the next without touching the video itself.
async fn upload_book_captions(
    state: &AppState,
    access_token: &str,
    audiobook_id: &str,
    video_id: &str,
    primary_chapters: &[DbChapter],
    primary_language: &str,
) {
    // Pull text for every translated language version of this audiobook,
    // grouped by language → (chapter_number → body_md).
    let by_language = match load_chapter_texts_by_language(state, audiobook_id).await {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, audiobook = %audiobook_id, "yt captions: load chapter texts failed");
            return;
        }
    };
    if by_language.is_empty() {
        return;
    }

    for (lang, texts) in &by_language {
        // Build (text, duration) tuples by walking the primary chapter
        // list — chapters missing in this language are simply omitted,
        // leaving a transparent gap on the caption timeline rather than
        // failing the upload.
        let inputs: Vec<(&str, u64)> = primary_chapters
            .iter()
            .filter_map(|c| {
                let dur = c.duration_ms.unwrap_or(0).max(0) as u64;
                if dur == 0 {
                    return None;
                }
                let body = texts.get(&c.number)?.trim();
                if body.is_empty() {
                    return None;
                }
                Some((body, dur))
            })
            .collect();
        if inputs.is_empty() {
            continue;
        }
        let srt = subtitles::build_srt_for_book(&inputs);
        if srt.trim().is_empty() {
            continue;
        }
        // YouTube uses the language code to drive the CC menu; the name
        // is just a display label per track. Tag the primary track plain
        // and other tracks with the locale so they're distinguishable in
        // YouTube Studio.
        let track_name = if lang == primary_language {
            "AidBooks".to_string()
        } else {
            format!("AidBooks ({lang})")
        };
        match upload::upload_caption(access_token, video_id, lang, &track_name, &srt).await {
            Ok(()) => info!(
                video_id,
                language = %lang,
                chapters = inputs.len(),
                "yt caption track uploaded"
            ),
            Err(e) => warn!(
                error = %e,
                video_id,
                language = %lang,
                "yt caption track upload failed"
            ),
        }
    }
}

/// Load every chapter row that has prose text, grouped by language and
/// keyed by chapter number. `BTreeMap` so the primary language sorts in
/// front of the alphabetised translations in logs.
async fn load_chapter_texts_by_language(
    state: &AppState,
    audiobook_id: &str,
) -> Result<std::collections::BTreeMap<String, std::collections::HashMap<i64, String>>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        language: String,
        number: i64,
        #[serde(default)]
        body_md: Option<String>,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT language, number, body_md FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
             ORDER BY language ASC, number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("yt caption load: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("yt caption load (decode): {e}")))?;
    let mut out: std::collections::BTreeMap<String, std::collections::HashMap<i64, String>> =
        std::collections::BTreeMap::new();
    for r in rows {
        let Some(body) = r.body_md else { continue };
        if body.trim().is_empty() {
            continue;
        }
        out.entry(r.language).or_default().insert(r.number, body);
    }
    Ok(out)
}

/// Set a custom thumbnail on a chapter video so the YouTube tile shows
/// the chapter's own art rather than whatever frame YouTube auto-picks
/// (which tends to converge on the same near-cover frame across every
/// chapter). Mirrors the first-frame logic in
/// `build_chapter_image_segments` — chapter art when present, cover
/// fallback otherwise — so the tile and the slideshow agree.
///
/// Best-effort: YouTube rejects custom thumbnails on un-verified
/// channels (phone-confirmation required), and the file may exceed
/// the 2 MiB cap. Both cases log a warning and leave YouTube to
/// auto-pick.
async fn upload_chapter_thumbnail(
    state: &AppState,
    access_token: &str,
    video_id: &str,
    chapter: &DbChapter,
    cover_path: &Path,
) {
    let storage = &state.config().storage_path;
    let path = chapter
        .chapter_art_path
        .as_deref()
        .map(|rel| storage.join(rel))
        .filter(|p| p.exists())
        .unwrap_or_else(|| cover_path.to_path_buf());

    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, ?path, video_id, "yt chapter thumbnail read failed");
            return;
        }
    };
    // YouTube caps custom thumbnails at 2 MiB. Skip oversize images
    // rather than upload-and-fail; the video tile falls back to the
    // auto-picked frame, which is at least the chapter's own art
    // because we put it at the head of the slideshow.
    const MAX_THUMBNAIL_BYTES: usize = 2 * 1024 * 1024;
    if bytes.len() > MAX_THUMBNAIL_BYTES {
        warn!(
            size = bytes.len(),
            ?path,
            video_id,
            "yt chapter thumbnail exceeds 2 MiB cap; skipping"
        );
        return;
    }
    let mime = crate::handlers::cover::detect_mime(&bytes);
    if let Err(e) = upload::upload_thumbnail(access_token, video_id, bytes, mime).await {
        warn!(
            error = %e,
            video_id,
            chapter = chapter.number,
            "yt chapter thumbnail upload failed"
        );
    }
}

async fn upload_chapter_captions(
    access_token: &str,
    video_id: &str,
    chapter: &DbChapter,
    language: &str,
) {
    let body = chapter.body_md.as_deref().unwrap_or("").trim();
    let dur = chapter.duration_ms.unwrap_or(0).max(0) as u64;
    if body.is_empty() || dur == 0 {
        return;
    }
    let srt = subtitles::build_srt_for_chapter(body, dur);
    if srt.trim().is_empty() {
        return;
    }
    if let Err(e) = upload::upload_caption(access_token, video_id, language, "AidBooks", &srt).await
    {
        warn!(
            error = %e,
            video_id,
            chapter = chapter.number,
            language,
            "yt chapter caption upload failed"
        );
    }
}

fn find_cover(state: &AppState, audiobook_id: &str) -> Option<PathBuf> {
    let dir = state.config().storage_path.join(audiobook_id);
    for ext in ["png", "jpg", "jpeg", "webp"] {
        let p = dir.join(format!("cover.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod hyperframes_tests {
    use super::*;

    fn book(title: &str) -> DbAudiobook {
        DbAudiobook {
            title: title.into(),
            topic: "test".into(),
            genre: None,
            language: None,
            is_short: Some(true),
            snippet_count: None,
        }
    }

    fn chapter(number: i64, title: &str, dur_ms: i64, synopsis: Option<&str>) -> DbChapter {
        DbChapter {
            number,
            title: title.into(),
            status: "done".into(),
            duration_ms: Some(dur_ms),
            synopsis: synopsis.map(str::to_string),
            chapter_art_path: None,
            body_md: None,
            paragraphs: None,
        }
    }

    #[test]
    fn allocate_steps_distributes_proportionally() {
        let chapters = vec![
            chapter(1, "A", 1000, None),
            chapter(2, "B", 5000, None),
            chapter(3, "C", 4000, None),
        ];
        let alloc = allocate_steps(&chapters, 10);
        assert_eq!(alloc.iter().sum::<usize>(), 10);
        assert!(alloc.iter().all(|&n| n >= 1));
        // Largest chapter gets the most slots.
        assert_eq!(alloc.iter().copied().max().unwrap(), alloc[1]);
    }

    #[test]
    fn allocate_steps_below_chapter_count_promotes_to_chapter_count() {
        let chapters = vec![
            chapter(1, "A", 1000, None),
            chapter(2, "B", 1000, None),
            chapter(3, "C", 1000, None),
            chapter(4, "D", 1000, None),
        ];
        // User asked for 2 steps but we have 4 chapters: every chapter
        // still gets a slot so coverage isn't lost.
        let alloc = allocate_steps(&chapters, 2);
        assert_eq!(alloc.len(), 4);
        assert!(alloc.iter().all(|&n| n >= 1));
        assert_eq!(alloc.iter().sum::<usize>(), 4);
    }

    #[test]
    fn split_into_sentences_respects_max() {
        let s = "First sentence. Second one! Third? Fourth here.";
        let out = split_into_sentences(s, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "First sentence.");
        assert_eq!(out[1], "Second one!");
    }

    #[test]
    fn truncate_sentence_cuts_at_word_boundary() {
        let s = "the quick brown fox jumps over the lazy dog";
        let t = truncate_sentence(s, 20);
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 21);
        assert!(!t.contains("fo…")); // should cut at a space
    }

    #[test]
    fn html_escape_quotes_dangerous_chars() {
        assert_eq!(
            html_escape("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn build_html_emits_one_scene_per_step_and_registers_timeline() {
        let b = book("My <Test> Book");
        let chapters = vec![
            chapter(1, "Opening", 4000, Some("Hello. World.")),
            chapter(2, "Middle", 6000, Some("More content here.")),
        ];
        let cover = PathBuf::from("/nonexistent/cover.png");
        let storage = PathBuf::from("/nonexistent/storage");
        let html = build_hyperframes_short_html("comp-id", &b, &chapters, &cover, &storage, 6, 10)
            .unwrap();
        // Composition wiring.
        assert!(html.contains(r#"data-composition-id="comp-id""#));
        assert!(html.contains(r#"data-duration="10""#));
        assert!(html.contains("window.__timelines['comp-id']"));
        // One <div class="scene"> per allocated step.
        assert_eq!(html.matches(r#"<div class="scene""#).count(), 6);
        // Book title is HTML-escaped (no raw `<` survives).
        assert!(html.contains("My &lt;Test&gt; Book"));
        // First scene gets the opener element.
        assert_eq!(html.matches(r#"class="opener""#).count(), 1);
    }

    #[test]
    fn build_html_falls_back_to_gradient_when_no_images_exist() {
        let b = book("Untitled");
        let chapters = vec![chapter(1, "Solo", 3000, None)];
        let cover = PathBuf::from("/nonexistent/cover.png");
        let storage = PathBuf::from("/nonexistent/storage");
        let html =
            build_hyperframes_short_html("c", &b, &chapters, &cover, &storage, 2, 4).unwrap();
        // Cover doesn't exist on disk, so every scene must fall back.
        assert!(html.contains("linear-gradient(135deg"));
        assert!(!html.contains("data:image/"));
    }
}
