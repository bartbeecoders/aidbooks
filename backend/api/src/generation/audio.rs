//! Audio generation: text → PCM → WAV + waveform.json, per chapter.
//!
//! Transitions:
//!   chapter.status: text_ready → running → audio_ready (or failed)
//!
//! Audiobook-level status (`chapters_running` → `audio_ready`/`failed`) is
//! owned by the Phase-5 Tts parent job, which fans out one `TtsChapter`
//! child per chapter and aggregates their outcomes. The per-chapter work
//! itself lives here and is what each child worker calls.
//!
//! Multi-voice mode: when the audiobook has `multi_voice_enabled = true`
//! and at least one entry in `voice_roles`, a per-chapter LLM extract
//! pass splits the prose into role-tagged segments
//! (`narrator | dialogue_male | dialogue_female`) — see
//! [`crate::generation::voices`]. The audio path then synthesises each
//! segment with the role's mapped voice and concatenates the PCM
//! buffers into one chapter WAV.

use std::collections::HashMap;

use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;
use tracing::{info, warn};

use crate::audio as audio_io;
use crate::generation::voices::{self, VoiceSegment};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ChapterRow {
    id: surrealdb::sql::Thing,
    number: i64,
    title: Option<String>,
    body_md: Option<String>,
    #[serde(default)]
    voice_segments: Option<Vec<VoiceSegment>>,
}

#[derive(Debug, Deserialize, Default)]
struct AudiobookMini {
    #[serde(default)]
    primary_voice: Option<surrealdb::sql::Thing>,
    #[serde(default)]
    multi_voice_enabled: Option<bool>,
    /// Free-form `{role: voice_id}` object — see migration 0038.
    /// Stored as plain strings (voice ids), not record refs, so the
    /// resolver doesn't pay for an extra round trip per role.
    #[serde(default)]
    voice_roles: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct VoiceMini {
    provider_voice_id: String,
}

#[derive(Debug, Deserialize)]
struct VoiceLabel {
    name: String,
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

    let book = load_audiobook_mini(state, audiobook_id).await?;
    let primary_voice = resolve_primary_voice(state, &book).await?;

    let multi_voice = book.multi_voice_enabled.unwrap_or(false)
        && book
            .voice_roles
            .as_ref()
            .map(|m| !m.is_empty())
            .unwrap_or(false);

    if multi_voice {
        synth_multi_voice(
            state,
            user,
            audiobook_id,
            &ch,
            body,
            &book,
            &primary_voice,
            language,
        )
        .await
    } else {
        synth_one(
            state,
            user,
            audiobook_id,
            &ch,
            body,
            &primary_voice,
            language,
        )
        .await
    }
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
            "SELECT id, number, title, body_md, status, voice_segments \
             FROM chapter \
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

async fn load_audiobook_mini(state: &AppState, audiobook_id: &str) -> Result<AudiobookMini> {
    let rows: Vec<AudiobookMini> = state
        .db()
        .inner()
        .query(format!(
            "SELECT primary_voice, multi_voice_enabled, voice_roles \
             FROM audiobook:`{audiobook_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("load audiobook mini: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load audiobook mini (decode): {e}")))?;
    Ok(rows.into_iter().next().unwrap_or_default())
}

/// Resolve the audiobook's primary voice (provider_voice_id). Falls
/// back to `Config.xai_default_voice` when no `primary_voice` is set.
async fn resolve_primary_voice(state: &AppState, book: &AudiobookMini) -> Result<String> {
    Ok(match book.primary_voice.as_ref() {
        Some(thing) => {
            let raw = thing.id.to_raw();
            resolve_provider_voice(state, &raw)
                .await
                .unwrap_or_else(|| state.config().xai_default_voice.clone())
        }
        None => state.config().xai_default_voice.clone(),
    })
}

/// Human-readable description of which voice(s) the next narration of
/// this audiobook will use, suitable for surfacing in progress events
/// (e.g. `"Eve"`, `"Eve / John / Anna"`, `"default voice"`).
///
/// Single-voice books return the primary voice's display name, falling
/// back to `"default voice"` when no `primary_voice` is set. Multi-voice
/// books return the three role voices joined with ` / ` in canonical
/// order (narrator, male dialogue, female dialogue), substituting the
/// narrator's voice for any role the user hasn't mapped — same fallback
/// the audio path itself applies, so what we render matches what we
/// label.
pub async fn chapter_voice_summary(state: &AppState, audiobook_id: &str) -> String {
    let book = match load_audiobook_mini(state, audiobook_id).await {
        Ok(b) => b,
        Err(_) => return "default voice".to_string(),
    };
    let multi_voice = book.multi_voice_enabled.unwrap_or(false)
        && book
            .voice_roles
            .as_ref()
            .map(|m| !m.is_empty())
            .unwrap_or(false);

    let primary_label = match book.primary_voice.as_ref() {
        Some(thing) => resolve_voice_label(state, &thing.id.to_raw())
            .await
            .unwrap_or_else(|| "default voice".to_string()),
        None => "default voice".to_string(),
    };

    if !multi_voice {
        return primary_label;
    }

    // Multi-voice: resolve each canonical role through the user's
    // mapping → fallback to narrator → fallback to the primary label.
    let roles = book.voice_roles.clone().unwrap_or_default();
    let mut out: HashMap<&'static str, String> = HashMap::new();
    for (role, voice_id) in &roles {
        let canonical = voices::canonical_role(role);
        if let Some(label) = resolve_voice_label(state, voice_id).await {
            out.insert(canonical, label);
        }
    }
    let narrator = out
        .get(voices::ROLE_NARRATOR)
        .cloned()
        .unwrap_or_else(|| primary_label.clone());
    let male = out
        .get(voices::ROLE_DIALOGUE_MALE)
        .cloned()
        .unwrap_or_else(|| narrator.clone());
    let female = out
        .get(voices::ROLE_DIALOGUE_FEMALE)
        .cloned()
        .unwrap_or_else(|| narrator.clone());
    format!("{narrator} / {male} / {female}")
}

/// Look up `voice:<id>.name`. Returns `None` when the row is missing
/// (admin removed the voice between save and narration). Used by
/// `chapter_voice_summary` for the user-facing stage label.
async fn resolve_voice_label(state: &AppState, voice_raw_id: &str) -> Option<String> {
    let rows: Vec<VoiceLabel> = state
        .db()
        .inner()
        .query(format!("SELECT name FROM voice:`{voice_raw_id}`"))
        .await
        .ok()?
        .take(0)
        .ok()?;
    rows.into_iter()
        .next()
        .map(|v| v.name.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Look up `voice:<id>.provider_voice_id`. Returns `None` when the
/// row is missing (e.g. an admin disabled or removed the voice between
/// the audiobook setting being saved and narration running).
async fn resolve_provider_voice(state: &AppState, voice_raw_id: &str) -> Option<String> {
    let rows: Vec<VoiceMini> = state
        .db()
        .inner()
        .query(format!(
            "SELECT provider_voice_id FROM voice:`{voice_raw_id}`"
        ))
        .await
        .ok()?
        .take(0)
        .ok()?;
    rows.into_iter().next().map(|v| v.provider_voice_id)
}

/// Multi-voice synthesis: load (or extract) the segment list, render
/// each segment with its role's mapped voice, concatenate the PCM
/// samples into one chapter WAV, and persist alongside the
/// single-voice path's outputs. Falls back to single-voice when the
/// extract pass produces nothing usable — the caller has already
/// verified that `multi_voice_enabled` is on and `voice_roles` is
/// non-empty.
#[allow(clippy::too_many_arguments)]
async fn synth_multi_voice(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    ch: &ChapterRow,
    body: &str,
    book: &AudiobookMini,
    primary_voice: &str,
    language: &str,
) -> Result<()> {
    let chapter_raw = ch.id.id.to_raw();
    set_chapter_status(state, &chapter_raw, "running").await?;

    // Extract once, cache forever (until translate clears it).
    let segments: Vec<VoiceSegment> = match ch.voice_segments.as_ref() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            let title = ch.title.as_deref().unwrap_or("");
            voices::extract_and_cache(
                state,
                user,
                audiobook_id,
                &chapter_raw,
                language,
                title,
                body,
            )
            .await
        }
    };
    if segments.is_empty() {
        // Extract degraded all the way to nothing — fall back to
        // single-voice rather than failing the chapter.
        warn!(
            audiobook = audiobook_id,
            chapter = ch.number,
            "voice_extract: no segments; falling back to single-voice"
        );
        return synth_one(state, user, audiobook_id, ch, body, primary_voice, language).await;
    }

    // Resolve `role -> provider_voice_id` once. Any role missing from
    // the audiobook's `voice_roles` falls back to the role labelled
    // `narrator`, then to the primary_voice. This way a half-mapped
    // configuration still produces something coherent.
    let role_voices = resolve_role_voices(state, book, primary_voice).await;

    let tts = state.tts().clone();
    let mut all_samples: Vec<i16> = Vec::new();
    let mut sample_rate_hz: u32 = 0;
    let mut total_chars: usize = 0;
    let mut any_mocked = false;

    for (idx, seg) in segments.iter().enumerate() {
        let voice = role_voices
            .get(seg.role.as_str())
            .cloned()
            .unwrap_or_else(|| primary_voice.to_string());
        let char_count = seg.text.chars().count();
        total_chars += char_count;

        let audio = match tts.synthesize(&seg.text, &voice, language).await {
            Ok(a) => a,
            Err(e) => {
                set_chapter_status(state, &chapter_raw, "failed").await.ok();
                log_tts_event(
                    state,
                    user,
                    Some(audiobook_id),
                    &voice,
                    char_count,
                    0,
                    Some(&e.to_string()),
                )
                .await?;
                return Err(e);
            }
        };
        if sample_rate_hz == 0 {
            sample_rate_hz = audio.sample_rate_hz;
        } else if audio.sample_rate_hz != sample_rate_hz {
            // All segments come from the same TTS provider in the same
            // call session, so this should never trip — but if a mid-
            // session config swap ever leaks through, fail loud rather
            // than mux mismatched rates.
            set_chapter_status(state, &chapter_raw, "failed").await.ok();
            return Err(Error::Other(anyhow::anyhow!(
                "tts segment {idx}: sample-rate mismatch ({} vs {})",
                audio.sample_rate_hz,
                sample_rate_hz,
            )));
        }
        any_mocked = any_mocked || audio.mocked;
        all_samples.extend_from_slice(&audio.samples);

        log_tts_event(
            state,
            user,
            Some(audiobook_id),
            &voice,
            char_count,
            audio.duration_ms(),
            None,
        )
        .await?;
    }

    if sample_rate_hz == 0 {
        sample_rate_hz = state.config().xai_sample_rate_hz;
    }

    let storage = state.config().storage_path.clone();
    let files = audio_io::write_chapter(
        &storage,
        audiobook_id,
        ch.number as u32,
        language,
        &all_samples,
        sample_rate_hz,
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

    info!(
        audiobook = audiobook_id,
        chapter = ch.number,
        segments = segments.len(),
        chars = total_chars,
        duration_ms = files.duration_ms,
        bytes = files.bytes,
        mocked = any_mocked,
        "chapter audio ready (multi-voice)"
    );
    Ok(())
}

/// Resolve `voice_roles` (a `{role: voice_id}` map) into
/// `{role: provider_voice_id}`. Roles whose voice can't be resolved
/// (deleted or never mapped) inherit the primary voice. Always
/// includes a `narrator` entry — it's the canonical fallback for
/// segments tagged with an unknown role.
async fn resolve_role_voices(
    state: &AppState,
    book: &AudiobookMini,
    primary_voice: &str,
) -> HashMap<&'static str, String> {
    let mut out: HashMap<&'static str, String> = HashMap::new();
    out.insert(voices::ROLE_NARRATOR, primary_voice.to_string());
    out.insert(voices::ROLE_DIALOGUE_MALE, primary_voice.to_string());
    out.insert(voices::ROLE_DIALOGUE_FEMALE, primary_voice.to_string());

    let Some(roles) = book.voice_roles.as_ref() else {
        return out;
    };
    for (role, voice_id) in roles {
        let canonical = voices::canonical_role(role);
        let resolved = resolve_provider_voice(state, voice_id).await;
        if let Some(provider_voice) = resolved {
            out.insert(canonical, provider_voice);
        } else {
            warn!(
                role = role,
                voice_id = voice_id,
                "voice_roles: voice not found; falling back to primary"
            );
        }
    }
    // After mapping, narrator-fallback for the dialogue roles when the
    // user only set the narrator: copy narrator's voice into any
    // role that's still pointing at the primary.
    let narrator_voice = out
        .get(voices::ROLE_NARRATOR)
        .cloned()
        .unwrap_or_else(|| primary_voice.to_string());
    for role in [voices::ROLE_DIALOGUE_MALE, voices::ROLE_DIALOGUE_FEMALE] {
        if !roles.contains_key(role) {
            out.insert(role, narrator_voice.clone());
        }
    }
    out
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
