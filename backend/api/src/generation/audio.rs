//! Audio generation: text → PCM → WAV + waveform.json, per chapter.
//!
//! Transitions:
//!   chapter.status: text_ready → running → audio_ready (or failed)
//!
//! Audiobook-level status (`chapters_running` → `audio_ready`/`failed`) is
//! owned by the Phase-5 Tts parent job, which fans out one `TtsChapter`
//! child per chapter and aggregates their outcomes. The per-chapter work
//! itself lives here and is what each child worker calls.

use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::info;

use crate::audio as audio_io;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ChapterRow {
    id: surrealdb::sql::Thing,
    number: i64,
    body_md: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AudiobookMini {
    primary_voice: Option<surrealdb::sql::Thing>,
}

#[derive(Debug, Deserialize)]
struct VoiceMini {
    provider_voice_id: String,
}

/// Generate audio for a single chapter by number, in the requested language.
pub async fn run_one_by_number(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    chapter_number: i64,
    language: &str,
) -> Result<()> {
    let ch = load_chapter_for(state, audiobook_id, chapter_number, language).await?;
    let body = ch
        .body_md
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Validation("chapter has no text to narrate".into()))?;
    let voice = resolve_voice(state, audiobook_id).await?;
    synth_one(state, user, audiobook_id, &ch, body, &voice, language).await
}

async fn synth_one(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    ch: &ChapterRow,
    body: &str,
    voice_id: &str,
    language: &str,
) -> Result<()> {
    let chapter_raw = ch.id.id.to_raw();
    set_chapter_status(state, &chapter_raw, "running").await?;

    let tts = state.tts().clone();
    let char_count = body.chars().count();
    let audio = match tts.synthesize(body, voice_id, language).await {
        Ok(a) => a,
        Err(e) => {
            set_chapter_status(state, &chapter_raw, "failed").await.ok();
            log_tts_event(
                state,
                user,
                Some(audiobook_id),
                "",
                char_count,
                0,
                Some(&e.to_string()),
            )
            .await?;
            return Err(e);
        }
    };

    let storage = state.config().storage_path.clone();
    let files = audio_io::write_chapter(
        &storage,
        audiobook_id,
        ch.number as u32,
        language,
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
        char_count,
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

async fn load_chapter_for(
    state: &AppState,
    audiobook_id: &str,
    number: i64,
    language: &str,
) -> Result<ChapterRow> {
    let rows: Vec<ChapterRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, number, body_md, status FROM chapter \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", number))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("load chapter: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapter (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id} chapter {number} ({language})"),
    })
}

/// Pick the voice for this audiobook. Falls back to
/// `Config.xai_default_voice` when no `primary_voice` is set.
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
    Ok(match voice_ref {
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
            rows.into_iter()
                .next()
                .map(|v| v.provider_voice_id)
                .unwrap_or_else(|| state.config().xai_default_voice.clone())
        }
        None => state.config().xai_default_voice.clone(),
    })
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

/// Append a TTS cost row to `generation_event` — `role="tts"`. Cost is
/// computed locally from `xai_tts_cost_per_1k_chars` × char count since
/// xAI's TTS endpoint doesn't return a billed cost the way OpenRouter does.
/// We persist the char count in `prompt_tokens` so the cost UI can
/// reconstruct it; `completion_tokens` carries duration_ms for the same
/// reason. Both fields are repurposed labels here, not real token counts.
#[allow(clippy::too_many_arguments)]
async fn log_tts_event(
    state: &AppState,
    user: &UserId,
    audiobook_id: Option<&str>,
    voice_id: &str,
    char_count: usize,
    duration_ms: u64,
    error: Option<&str>,
) -> Result<()> {
    let event_id = uuid::Uuid::new_v4().simple().to_string();
    let audiobook_set = match audiobook_id {
        Some(id) => format!(", audiobook: audiobook:`{id}`"),
        None => String::new(),
    };
    // $/1k chars × char_count / 1000. Mock paths (or admin sets price = 0)
    // stay at $0 — the UI shows that as "free" rather than misleading "$0".
    let cost_usd = if error.is_none() {
        (char_count as f64) * state.config().xai_tts_cost_per_1k_chars / 1000.0
    } else {
        0.0
    };
    // Record the provider voice id in `error` slot when success so the admin
    // panel can show "Eve: 34.2s" without adding columns now.
    let note = if error.is_none() {
        Some(format!(
            "voice={voice_id} duration_ms={duration_ms} chars={char_count}"
        ))
    } else {
        error.map(str::to_string)
    };
    let sql = format!(
        r#"CREATE generation_event:`{event_id}` CONTENT {{
            user: user:`{user}`,
            llm: llm:`claude_haiku_4_5`,
            role: "tts",
            prompt_tokens: $chars,
            completion_tokens: $duration_ms,
            cost_usd: $cost,
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
        .bind(("chars", char_count as i64))
        .bind(("duration_ms", duration_ms as i64))
        .bind(("cost", cost_usd))
        .bind(("success", error.is_none()))
        .bind(("error", note))
        .await
        .map_err(|e| Error::Database(format!("log tts event: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("log tts event: {e}")))?;
    Ok(())
}
