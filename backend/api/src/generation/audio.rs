//! Audio generation: text → PCM → WAV + waveform.json, per chapter.
//!
//! Transitions:
//!   chapter.status: text_ready → running → audio_ready (or failed)
//!   audiobook.status:
//!       text_ready → chapters_running* → audio_ready (or failed)
//!
//!  * we reuse `chapters_running` for the audio pass too — it's accurate
//!    enough for the UI and avoids a schema change. Phase 5's job runner
//!    will introduce finer-grained status if the plan needs it.

use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::{error, info};

use crate::audio as audio_io;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ChapterRow {
    id: surrealdb::sql::Thing,
    number: i64,
    body_md: Option<String>,
    status: String,
}

#[derive(Debug, Deserialize)]
struct AudiobookMini {
    primary_voice: Option<surrealdb::sql::Thing>,
}

#[derive(Debug, Deserialize)]
struct VoiceMini {
    provider_voice_id: String,
}

/// Generate audio for every chapter in `text_ready` (or `failed`), in order.
/// Flips the audiobook between `chapters_running` → `audio_ready` / `failed`.
pub async fn run_all(state: &AppState, user: &UserId, audiobook_id: &str) -> Result<()> {
    set_audiobook_status(state, audiobook_id, "chapters_running").await?;

    let chapters = load_chapters(state, audiobook_id).await?;
    let voice = resolve_voice(state, audiobook_id).await?;

    let mut any_failed = false;
    for ch in chapters {
        if !matches!(ch.status.as_str(), "text_ready" | "failed" | "audio_ready") {
            continue;
        }
        let body = match ch.body_md.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => {
                error!(chapter = ch.number, "chapter body empty; skipping");
                any_failed = true;
                continue;
            }
        };
        if let Err(e) = synth_one(state, user, audiobook_id, &ch, body, &voice).await {
            error!(chapter = ch.number, error = %e, "audio generation failed");
            any_failed = true;
        }
    }

    set_audiobook_status(
        state,
        audiobook_id,
        if any_failed { "failed" } else { "audio_ready" },
    )
    .await?;
    Ok(())
}

/// Regenerate audio for a single chapter by number.
pub async fn run_one_by_number(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    chapter_number: i64,
) -> Result<()> {
    let chapters = load_chapters(state, audiobook_id).await?;
    let ch = chapters
        .into_iter()
        .find(|c| c.number == chapter_number)
        .ok_or(Error::NotFound {
            resource: format!("audiobook:{audiobook_id} chapter {chapter_number}"),
        })?;
    let body = ch
        .body_md
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Validation("chapter has no text to narrate".into()))?;
    let voice = resolve_voice(state, audiobook_id).await?;
    synth_one(state, user, audiobook_id, &ch, body, &voice).await
}

async fn synth_one(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    ch: &ChapterRow,
    body: &str,
    voice_id: &str,
) -> Result<()> {
    let chapter_raw = ch.id.id.to_raw();
    set_chapter_status(state, &chapter_raw, "running").await?;

    let tts = state.tts().clone();
    let audio = match tts.synthesize(body, voice_id).await {
        Ok(a) => a,
        Err(e) => {
            set_chapter_status(state, &chapter_raw, "failed").await.ok();
            log_tts_event(state, user, Some(audiobook_id), "", 0, Some(&e.to_string())).await?;
            return Err(e);
        }
    };

    let storage = state.config().storage_path.clone();
    let files = audio_io::write_chapter(
        &storage,
        audiobook_id,
        ch.number as u32,
        &audio.samples,
        audio.sample_rate_hz,
    )?;

    state
        .db()
        .inner()
        .query(format!(
            r#"UPDATE chapter:`{chapter_raw}` SET
                audio_path = $audio_path,
                duration_ms = $duration_ms,
                status = "audio_ready"
            "#
        ))
        .bind(("audio_path", files.wav_path.display().to_string()))
        .bind(("duration_ms", files.duration_ms as i64))
        .await
        .map_err(|e| Error::Database(format!("persist chapter audio: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("persist chapter audio: {e}")))?;

    log_tts_event(
        state,
        user,
        Some(audiobook_id),
        voice_id,
        files.duration_ms,
        None,
    )
    .await?;

    info!(
        audiobook = audiobook_id,
        chapter = ch.number,
        voice = voice_id,
        duration_ms = files.duration_ms,
        bytes = files.bytes,
        mocked = audio.mocked,
        "chapter audio ready"
    );
    Ok(())
}

async fn load_chapters(state: &AppState, audiobook_id: &str) -> Result<Vec<ChapterRow>> {
    let rows: Vec<ChapterRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, number, body_md, status FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` ORDER BY number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("load chapters: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapters (decode): {e}")))?;
    Ok(rows)
}

/// Pick the voice for this audiobook: use the audiobook's `primary_voice`
/// if set, otherwise fall back to `Config.xai_default_voice`.
async fn resolve_voice(state: &AppState, audiobook_id: &str) -> Result<String> {
    let rows: Vec<AudiobookMini> = state
        .db()
        .inner()
        .query(format!(
            "SELECT primary_voice FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("resolve voice: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("resolve voice (decode): {e}")))?;
    let voice_ref = rows.into_iter().next().and_then(|m| m.primary_voice);
    match voice_ref {
        Some(thing) => {
            let raw = thing.id.to_raw();
            let rows: Vec<VoiceMini> = state
                .db()
                .inner()
                .query(format!("SELECT provider_voice_id FROM voice:`{raw}`"))
                .await
                .map_err(|e| Error::Database(format!("load voice: {e}")))?
                .take(0)
                .map_err(|e| Error::Database(format!("load voice (decode): {e}")))?;
            Ok(rows
                .into_iter()
                .next()
                .map(|v| v.provider_voice_id)
                .unwrap_or_else(|| state.config().xai_default_voice.clone()))
        }
        None => Ok(state.config().xai_default_voice.clone()),
    }
}

async fn set_audiobook_status(state: &AppState, audiobook_id: &str, status: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET status = $status"
        ))
        .bind(("status", status.to_string()))
        .await
        .map_err(|e| Error::Database(format!("set audiobook status: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set audiobook status: {e}")))?;
    Ok(())
}

async fn set_chapter_status(state: &AppState, chapter_raw: &str, status: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE chapter:`{chapter_raw}` SET status = $status"
        ))
        .bind(("status", status.to_string()))
        .await
        .map_err(|e| Error::Database(format!("set chapter status: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set chapter status: {e}")))?;
    Ok(())
}

/// Append a TTS cost row to `generation_event` — `role="tts"`. We
/// currently don't bill against an LLM row, so `llm:_default_` is used as
/// a placeholder; Phase 9 (billing) will replace this with a voice-priced
/// table keyed on provider + minutes.
async fn log_tts_event(
    state: &AppState,
    user: &UserId,
    audiobook_id: Option<&str>,
    voice_id: &str,
    duration_ms: u64,
    error: Option<&str>,
) -> Result<()> {
    let event_id = uuid::Uuid::new_v4().simple().to_string();
    let audiobook_set = match audiobook_id {
        Some(id) => format!(", audiobook: audiobook:`{id}`"),
        None => String::new(),
    };
    // Record the provider voice id in `error` slot when success so the admin
    // panel can show "Eve: 34.2s" without adding columns now.
    let note = if error.is_none() {
        Some(format!("voice={voice_id} duration_ms={duration_ms}"))
    } else {
        error.map(str::to_string)
    };
    let sql = format!(
        r#"CREATE generation_event:`{event_id}` CONTENT {{
            user: user:`{user}`,
            llm: llm:`claude_haiku_4_5`,
            role: "tts",
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: 0.0,
            success: $success,
            error: $error
            {audiobook_set}
        }}"#,
        user = user.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("success", error.is_none()))
        .bind(("error", note))
        .await
        .map_err(|e| Error::Database(format!("log tts event: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("log tts event: {e}")))?;
    Ok(())
}
