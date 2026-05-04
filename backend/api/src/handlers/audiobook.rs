//! Audiobook CRUD + content-generation triggers.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use listenai_core::domain::{AudiobookLength, AudiobookStatus, ChapterStatus, JobKind};
use listenai_core::id::{AudiobookId, ChapterId, UserId};
use listenai_core::{Error, Result};
use listenai_jobs::repo::EnqueueRequest;
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::generation::{audio as audio_gen, chapter as chapter_gen, outline as outline_gen};
use crate::idempotency::{self, IdempotencyKey};
use crate::state::AppState;
use tracing::warn;

// -------------------------------------------------------------------------
// DTOs
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateAudiobookRequest {
    #[validate(length(min = 3, max = 500))]
    pub topic: String,
    pub length: AudiobookLength,
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    /// Optional user-supplied bucket for the library view (e.g. "Bedtime
    /// stories"). Free-text, max 60 chars. Distinct from `genre`, which
    /// is the AI-content type.
    #[validate(length(max = 60))]
    pub category: Option<String>,
    /// Optional voice id from `/voices`. If omitted, the TTS layer falls
    /// back to the configured `xai_default_voice`.
    #[validate(length(min = 1, max = 64))]
    pub voice_id: Option<String>,
    /// Optional pre-generated cover artwork. Raw base64 (no `data:` prefix);
    /// produced by `POST /cover-art/preview`. Persisted to disk and served
    /// later via `GET /audiobook/:id/cover`.
    pub cover_image_base64: Option<String>,
    /// BCP-47 language code, e.g. `"en"`, `"nl"`, `"de"`. Drives both LLM
    /// content generation and TTS narration. Defaults to `"en"`.
    #[validate(length(min = 2, max = 8))]
    pub language: Option<String>,
    /// Visual style for cover + chapter artwork (e.g. `"watercolor"`,
    /// `"cartoon"`, `"realistic"`). Stored on the audiobook so subsequent
    /// regenerations stay consistent. Free-text; the frontend offers a
    /// curated list.
    #[validate(length(max = 60))]
    pub art_style: Option<String>,
    /// Pin a specific LLM (`llm:<id>`) for cover + chapter art generation.
    /// `None` falls back to whichever model is marked default-for cover_art.
    #[validate(length(max = 64))]
    pub cover_llm_id: Option<String>,
    /// Tiles to generate per *visualizable* paragraph (after the LLM
    /// extract pass picks paragraphs that have visual content). `None` /
    /// `0` = no paragraph-level art (only chapter cover tiles). Capped
    /// at 3. Auto-pipeline must include `cover: true` for these to run.
    pub images_per_paragraph: Option<u32>,
    /// Optional one-shot pipeline: after the synchronous outline runs,
    /// the server can chain chapter writing → narration → YouTube publish
    /// without further user action. `None` = the legacy step-by-step flow.
    pub auto_pipeline: Option<AutoPipelineRequest>,
    /// When `true`, generate the book as a YouTube Short: a single
    /// chapter ≤ 90 s of narration with a vertical 9:16 cover. The
    /// publish step renders a vertical 1080×1920 video and forces
    /// `mode = single`.
    pub is_short: Option<bool>,
    /// Optional multi-voice narration toggle. When `true`, the
    /// per-chapter narration job runs an extra LLM extract pass to
    /// split prose into role-tagged segments and renders each
    /// segment with its mapped voice from `voice_roles`. Defaults to
    /// `false`.
    pub multi_voice_enabled: Option<bool>,
    /// Voice mapping per role for multi-voice narration. Keys are
    /// canonical role names (`narrator`, `dialogue_male`,
    /// `dialogue_female`); values are voice ids (e.g. `"eve"`).
    pub voice_roles: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
pub struct AutoPipelineRequest {
    /// Enqueue the chapter-writing job immediately after outline lands.
    #[serde(default)]
    pub chapters: bool,
    /// Enqueue a cover-art job alongside chapter writing. Also drives
    /// per-chapter art: when this is true and chapters is true, one
    /// chapter-art job fans out per chapter after the prose lands.
    /// Ignored when the caller already supplied `cover_image_base64`
    /// (for the main cover only — chapter art still runs).
    #[serde(default)]
    pub cover: bool,
    /// After chapters finish, enqueue narration. Ignored when `chapters`
    /// is false (no chapters → nothing to narrate).
    #[serde(default)]
    pub audio: bool,
    /// After narration finishes, enqueue a YouTube publish. `None` = no
    /// publish step. Requires the user to have a YouTube channel
    /// connected; otherwise the publish step skips with a warn.
    #[serde(default)]
    pub publish: Option<AutoPublishRequest>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
pub struct AutoPublishRequest {
    /// `single` (one concatenated video, default) or `playlist`.
    pub mode: Option<String>,
    /// `private`, `unlisted`, `public`. Defaults to `private`.
    pub privacy_status: Option<String>,
    /// When true, the publish stops after encoding so the user can
    /// preview before approving.
    #[serde(default)]
    pub review: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookSummary {
    pub id: AudiobookId,
    pub title: String,
    pub topic: String,
    pub genre: Option<String>,
    /// Free-text user grouping (e.g. "Bedtime stories"). `None` means the
    /// book is in the implicit "Uncategorized" bucket.
    pub category: Option<String>,
    /// Podcast id (`podcast:<id>`) the book has been assigned to. `None`
    /// when the book is unassigned. The podcast row owns the title +
    /// description + cover art used by the future YouTube playlist.
    pub podcast_id: Option<String>,
    pub length: AudiobookLength,
    pub status: AudiobookStatus,
    /// `true` when a cover image has been generated and is available at
    /// `GET /audiobook/:id/cover`.
    pub has_cover: bool,
    /// BCP-47 code for the audiobook's *primary* (originally generated)
    /// language. Stays stable; `available_languages` grows as translations
    /// are added.
    pub language: String,
    /// Every language this audiobook has chapters in (always includes the
    /// primary language). Drives the language switcher on the detail/player
    /// pages.
    pub available_languages: Vec<String>,
    /// Voice picked for narration (`None` when the server default is used).
    /// Frontend resolves the human-readable name via `/voices`.
    pub voice_id: Option<String>,
    /// Visual style applied to cover + chapter artwork. `None` falls back to
    /// the generator default (currently "cinematic").
    pub art_style: Option<String>,
    /// LLM pinned for cover + chapter art generation. `None` falls back to
    /// the picker (whichever model is default-for `cover_art`).
    pub cover_llm_id: Option<String>,
    /// Tiles per visual paragraph (extracted by the LLM scene pass). `0`
    /// = no paragraph art, only chapter cover tiles.
    pub images_per_paragraph: u32,
    /// X.ai TTS speech-tag palette suggested by the outline LLM. The
    /// chapter writer embeds these inline in `body_md` (e.g. `[pause]`,
    /// `<whisper>...</whisper>`); the X.ai TTS endpoint consumes them
    /// directly from the text. Empty = plain narration.
    pub tags: Vec<String>,
    /// Total narration runtime for the primary-language chapters, in
    /// milliseconds. `None` until at least one chapter has finished
    /// narration. Library views use this to display + filter on length.
    pub duration_ms: Option<u64>,
    /// `true` when the book is rendered as a YouTube Short (≤ 90 s of
    /// narration, vertical 9:16 artwork, single-video upload).
    pub is_short: bool,
    /// `true` when this audiobook narrates with per-role voices
    /// (narrator + dialogue_male + dialogue_female). The role-to-voice
    /// map lives in `voice_roles`.
    pub multi_voice_enabled: bool,
    /// `{role: voice_id}` map. Empty when multi-voice isn't
    /// configured. Always serialised so the UI can render the picker
    /// without a separate fetch.
    pub voice_roles: std::collections::HashMap<String, String>,
    /// LLM verdict on whether the topic is STEM (math / physics /
    /// chemistry / biology / CS / engineering). Set during outline
    /// generation. `None` until the outline LLM runs (legacy rows or
    /// fresh drafts).
    pub stem_detected: Option<bool>,
    /// Explicit user override that wins over `stem_detected`. `None`
    /// means "trust the LLM verdict". Settable through
    /// `PATCH /audiobook/:id` with three states: absent (don't
    /// touch), null (clear), or bool (force).
    pub stem_override: Option<bool>,
    /// Effective STEM flag the renderer actually uses:
    /// `stem_override.unwrap_or(stem_detected.unwrap_or(false))`.
    /// Pre-computed here so the frontend doesn't have to repeat the
    /// fallback logic.
    pub is_stem: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookDetail {
    #[serde(flatten)]
    pub summary: AudiobookSummary,
    pub chapters: Vec<ChapterSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChapterSummary {
    pub id: ChapterId,
    pub number: u32,
    pub title: String,
    pub synopsis: Option<String>,
    pub target_words: Option<u32>,
    pub body_md: Option<String>,
    pub status: ChapterStatus,
    pub has_art: bool,
    /// Paragraph illustration metadata. One entry per paragraph the
    /// splitter produced (in body order); `image_count` reflects how many
    /// tiles have been generated so far at
    /// `GET /audiobook/:id/chapter/:n/paragraph/:p/image/:i` (1-based).
    /// Empty when paragraph illustration wasn't requested for this book.
    pub paragraphs: Vec<ParagraphSummary>,
    /// WAV duration in milliseconds, populated once the chapter has been
    /// narrated. The player uses this to render a whole-book progress bar.
    pub duration_ms: Option<u64>,
    /// Which language version this chapter belongs to.
    pub language: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ParagraphSummary {
    /// Index into the chapter's paragraph array (stable across regenerations).
    pub index: u32,
    /// Character count of the paragraph body. Used by the player to
    /// time-slot the slideshow proportionally to chapter duration.
    pub char_count: u32,
    /// `true` when the LLM extract pass identified visual content here
    /// (i.e. `scene_description` is set). Non-visual paragraphs are
    /// preserved in the array so indices stay stable, but they never
    /// get tile jobs enqueued.
    pub is_visual: bool,
    /// Tiles persisted so far for this paragraph (range `1..=image_count`
    /// addressable on the stream endpoint).
    pub image_count: u32,
    /// Phase G — diagram template id chosen by the per-paragraph
    /// visual classifier (`function_plot`, `free_body`, …). `None`
    /// for prose paragraphs and for non-STEM books that never ran the
    /// classifier. Frontend uses this to badge chapters with diagram
    /// counts and to drive the future Manim render path.
    pub visual_kind: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookList {
    pub items: Vec<AudiobookSummary>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateAudiobookRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[validate(length(max = 40))]
    pub genre: Option<String>,
    /// Pass `Some("Bedtime stories")` to set or change; pass an empty
    /// string to clear (book moves to "Uncategorized").
    #[validate(length(max = 60))]
    pub category: Option<String>,
    /// Podcast id to assign this audiobook to. Pass an empty string to
    /// clear (book is unassigned). The id must reference a podcast owned
    /// by the same user.
    #[validate(length(max = 64))]
    pub podcast_id: Option<String>,
    /// Pass `Some("eve")` to change the narrator. Re-narrate the audiobook
    /// after changing this for the new voice to take effect on existing
    /// audio files.
    #[validate(length(min = 1, max = 64))]
    pub voice_id: Option<String>,
    /// Pass `Some("watercolor")` to change the artwork style; pass an empty
    /// string to clear the override (falls back to the server default on the
    /// next regeneration).
    #[validate(length(max = 60))]
    pub art_style: Option<String>,
    /// Pass `Some("gemini_flash_image")` to pin a specific image model for
    /// cover + chapter art; pass an empty string to fall back to the picker.
    #[validate(length(max = 64))]
    pub cover_llm_id: Option<String>,
    /// New target count of tiles per visual paragraph (0..=3). `0`
    /// clears the override. Existing on-disk paragraph images stay on
    /// disk but only the first N per paragraph are surfaced in the
    /// chapter summary.
    pub images_per_paragraph: Option<u32>,
    /// Toggle YouTube Short mode. Existing chapters / cover art are
    /// not auto-regenerated — flip this before regenerating outline +
    /// cover to take effect.
    pub is_short: Option<bool>,
    /// Toggle multi-voice narration. When `true`, the next narration
    /// of any chapter runs the LLM extract pass to split prose into
    /// role-tagged segments and renders each segment with the role's
    /// mapped voice from `voice_roles`. Switching this on/off doesn't
    /// re-render existing audio — call `regenerate-audio` per chapter
    /// or use the audiobook-level audio job to apply the change.
    pub multi_voice_enabled: Option<bool>,
    /// Voice mapping per role. Keys are canonical role names
    /// (`narrator`, `dialogue_male`, `dialogue_female`); values are
    /// voice ids (e.g. `"eve"`). Roles missing from the map fall back
    /// to the narrator (or the primary voice if narrator isn't
    /// mapped). Pass an empty object to clear.
    pub voice_roles: Option<std::collections::HashMap<String, String>>,
    /// User override of the LLM-detected STEM flag. Three-state:
    /// absent (field not in body) = don't change; JSON `null` =
    /// clear the override (use LLM verdict); `true` / `false` =
    /// force the value. We use `serde_json::Value` because plain
    /// `Option<Option<bool>>` collapses absent and null into the
    /// same shape on the wire.
    #[serde(default)]
    pub stem_override: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateChapterRequest {
    #[validate(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[validate(length(max = 2_000))]
    pub synopsis: Option<String>,
    pub body_md: Option<String>,
}

// -------------------------------------------------------------------------
// Row types (private)
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DbAudiobook {
    id: Thing,
    owner: Thing,
    title: String,
    topic: String,
    genre: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    podcast: Option<Thing>,
    length: String,
    status: String,
    cover_path: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    primary_voice: Option<Thing>,
    #[serde(default)]
    art_style: Option<String>,
    #[serde(default)]
    cover_llm_id: Option<String>,
    #[serde(default)]
    images_per_paragraph: Option<i64>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    is_short: Option<bool>,
    #[serde(default)]
    multi_voice_enabled: Option<bool>,
    #[serde(default)]
    voice_roles: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    stem_detected: Option<bool>,
    #[serde(default)]
    stem_override: Option<bool>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct DbChapter {
    id: Thing,
    number: i64,
    title: String,
    synopsis: Option<String>,
    target_words: Option<i64>,
    body_md: Option<String>,
    status: String,
    #[serde(default)]
    chapter_art_path: Option<String>,
    #[serde(default)]
    paragraphs: Option<Vec<DbParagraph>>,
    duration_ms: Option<i64>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DbParagraph {
    #[serde(default)]
    pub(crate) index: i64,
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) char_count: Option<i64>,
    #[serde(default)]
    pub(crate) scene_description: Option<String>,
    #[serde(default)]
    pub(crate) image_paths: Vec<String>,
    /// Phase G — diagram template id ("function_plot", "free_body",
    /// …) the LLM picked for this paragraph. `None` for prose
    /// paragraphs and for non-STEM books that never ran the
    /// classifier.
    #[serde(default)]
    pub(crate) visual_kind: Option<String>,
    /// Template-specific parameters. Free-form JSON because each
    /// `visual_kind` owns its own param schema; the renderer
    /// validates per-template at draw time. Persisted now (G.2) and
    /// consumed by the Manim sidecar in G.5.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) visual_params: Option<serde_json::Value>,
    /// Phase H — bespoke Manim code block, only set when
    /// `visual_kind == "custom_manim"`. The publisher reads it via
    /// `load_paragraph_tiles`; the chapter detail endpoint exposes
    /// just a "has manim code" boolean to the frontend so we don't
    /// blow up the JSON payload with full source bodies.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) manim_code: Option<String>,
}

impl DbAudiobook {
    fn to_summary(&self) -> Result<AudiobookSummary> {
        Ok(AudiobookSummary {
            id: AudiobookId(self.id.id.to_raw()),
            title: self.title.clone(),
            topic: self.topic.clone(),
            genre: self.genre.clone(),
            category: self
                .category
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            podcast_id: self.podcast.as_ref().map(|t| t.id.to_raw()),
            length: parse_length(&self.length)?,
            status: parse_status(&self.status)?,
            has_cover: self
                .cover_path
                .as_deref()
                .map(|p| !p.trim().is_empty())
                .unwrap_or(false),
            language: self
                .language
                .clone()
                .unwrap_or_else(|| "en".to_string()),
            // Filled in by `enrich_summary` once the chapter set is loaded.
            available_languages: Vec::new(),
            voice_id: self.primary_voice.as_ref().map(|t| t.id.to_raw()),
            art_style: self
                .art_style
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            cover_llm_id: self
                .cover_llm_id
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            images_per_paragraph: self
                .images_per_paragraph
                .map(|n| n.clamp(0, 3) as u32)
                .unwrap_or(0),
            tags: self.tags.clone().unwrap_or_default(),
            // Filled in by the list/detail loaders once chapter durations
            // are aggregated. `None` here means "not yet computed".
            duration_ms: None,
            is_short: self.is_short.unwrap_or(false),
            multi_voice_enabled: self.multi_voice_enabled.unwrap_or(false),
            voice_roles: self.voice_roles.clone().unwrap_or_default(),
            stem_detected: self.stem_detected,
            stem_override: self.stem_override,
            // Effective STEM = override > detected > false.
            is_stem: self
                .stem_override
                .unwrap_or_else(|| self.stem_detected.unwrap_or(false)),
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }

    fn owner_id(&self) -> UserId {
        UserId(self.owner.id.to_raw())
    }
}

impl DbChapter {
    fn to_summary(&self) -> Result<ChapterSummary> {
        Ok(ChapterSummary {
            id: ChapterId(self.id.id.to_raw()),
            number: self.number as u32,
            title: self.title.clone(),
            synopsis: self.synopsis.clone(),
            target_words: self.target_words.map(|w| w as u32),
            body_md: self.body_md.clone(),
            status: parse_chapter_status(&self.status)?,
            has_art: self
                .chapter_art_path
                .as_deref()
                .map(|p| !p.trim().is_empty())
                .unwrap_or(false),
            paragraphs: self
                .paragraphs
                .as_ref()
                .map(|ps| {
                    ps.iter()
                        .map(|p| ParagraphSummary {
                            index: p.index.max(0) as u32,
                            char_count: p
                                .char_count
                                .unwrap_or_else(|| p.text.chars().count() as i64)
                                .max(0) as u32,
                            is_visual: p
                                .scene_description
                                .as_deref()
                                .map(|s| !s.trim().is_empty())
                                .unwrap_or(false),
                            image_count: p
                                .image_paths
                                .iter()
                                .filter(|s| !s.trim().is_empty())
                                .count() as u32,
                            visual_kind: p
                                .visual_kind
                                .clone()
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            duration_ms: self.duration_ms.map(|d| d.max(0) as u64),
            language: self
                .language
                .clone()
                .unwrap_or_else(|| "en".to_string()),
        })
    }
}

fn parse_length(s: &str) -> Result<AudiobookLength> {
    match s {
        "short" => Ok(AudiobookLength::Short),
        "medium" => Ok(AudiobookLength::Medium),
        "long" => Ok(AudiobookLength::Long),
        other => Err(Error::Database(format!("unknown length `{other}`"))),
    }
}

fn parse_status(s: &str) -> Result<AudiobookStatus> {
    Ok(match s {
        "draft" => AudiobookStatus::Draft,
        "outline_pending" => AudiobookStatus::OutlinePending,
        "outline_ready" => AudiobookStatus::OutlineReady,
        "chapters_running" => AudiobookStatus::ChaptersRunning,
        "text_ready" => AudiobookStatus::TextReady,
        "audio_ready" => AudiobookStatus::AudioReady,
        "failed" => AudiobookStatus::Failed,
        other => return Err(Error::Database(format!("unknown status `{other}`"))),
    })
}

fn parse_chapter_status(s: &str) -> Result<ChapterStatus> {
    Ok(match s {
        "pending" => ChapterStatus::Pending,
        "running" => ChapterStatus::Running,
        "text_ready" => ChapterStatus::TextReady,
        "audio_ready" => ChapterStatus::AudioReady,
        "failed" => ChapterStatus::Failed,
        other => return Err(Error::Database(format!("unknown chapter status `{other}`"))),
    })
}

fn length_to_str(l: AudiobookLength) -> &'static str {
    match l {
        AudiobookLength::Short => "short",
        AudiobookLength::Medium => "medium",
        AudiobookLength::Long => "long",
    }
}

// -------------------------------------------------------------------------
// POST /audiobook  — creates draft + runs outline synchronously
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook",
    tag = "audiobook",
    request_body = CreateAudiobookRequest,
    responses(
        (status = 200, description = "Outline ready", body = AudiobookDetail),
        (status = 400, description = "Validation failed"),
        (status = 401, description = "Unauthenticated"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Json(body): Json<CreateAudiobookRequest>,
) -> ApiResult<Json<AudiobookDetail>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;

    let id = Uuid::new_v4().simple().to_string();
    let genre = body.genre.clone().unwrap_or_default();

    // Pick + validate language. Default to "en" so old clients keep working.
    let language = body
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("en")
        .to_string();
    if !is_supported_language(&language) {
        return Err(Error::Validation(format!(
            "unsupported language `{language}`"
        ))
        .into());
    }

    // Validate voice_id refers to an enabled voice — fail fast with a 400
    // rather than letting a bad pick surface as a TTS failure 30 s later.
    let voice_id = match body.voice_id.as_deref() {
        Some(v) if !v.trim().is_empty() => Some(assert_voice_enabled(&state, v.trim()).await?),
        _ => None,
    };

    let voice_clause = match &voice_id {
        Some(vid) => format!("primary_voice: voice:`{vid}`,"),
        None => String::new(),
    };
    let art_style = body
        .art_style
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let cover_llm_id = body
        .cover_llm_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Cap at 3 so a runaway client can't queue dozens of image jobs per
    // paragraph. Combined with the per-book paragraph cap inside the
    // ChapterParagraphs handler, this bounds total image cost.
    let images_per_paragraph: Option<i64> = body.images_per_paragraph.map(|n| n.min(3) as i64);
    // Validate + normalise pipeline upfront so a bad value never lands in
    // the row. Empty pipeline (no chapters/audio/publish) is the same as
    // not requesting one at all — drop it.
    let auto_pipeline = match body.auto_pipeline.as_ref() {
        Some(p) => normalise_auto_pipeline(p, body.is_short.unwrap_or(false))?,
        None => None,
    };
    // Bind the typed struct directly. Going through `serde_json::Value`
    // here doesn't round-trip cleanly through SurrealDB's `option<object>`
    // — inner fields silently come back as defaults — so the auto-pipeline
    // chain would skip narration + chapter art on the read-back. See the
    // analogous workaround in `jobs/publishers/youtube.rs`.
    let category = body
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(c) = &category {
        assert_category_exists(&state, c).await?;
    }
    let is_short = body.is_short.unwrap_or(false);
    let multi_voice_enabled = body.multi_voice_enabled.unwrap_or(false);
    let voice_roles_json: Option<serde_json::Value> = body
        .voice_roles
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));
    let sql = format!(
        r#"CREATE audiobook:`{id}` CONTENT {{
            owner: user:`{user_id}`,
            title: "Untitled",
            topic: $topic,
            genre: $genre,
            category: $category,
            length: $length,
            language: $language,
            art_style: $art_style,
            cover_llm_id: $cover_llm_id,
            images_per_paragraph: $images_per_paragraph,
            auto_pipeline: $auto_pipeline,
            is_short: $is_short,
            multi_voice_enabled: $multi_voice_enabled,
            voice_roles: $voice_roles,
            {voice_clause}
            status: "draft"
        }}"#,
        user_id = user.id.0,
    );
    state
        .db()
        .inner()
        .query(sql)
        .bind(("topic", body.topic.trim().to_string()))
        .bind((
            "genre",
            if genre.is_empty() {
                None
            } else {
                Some(genre.clone())
            },
        ))
        .bind(("category", category))
        .bind(("length", length_to_str(body.length).to_string()))
        .bind(("language", language.clone()))
        .bind(("art_style", art_style))
        .bind(("cover_llm_id", cover_llm_id))
        .bind(("images_per_paragraph", images_per_paragraph))
        .bind(("auto_pipeline", auto_pipeline.clone()))
        .bind(("is_short", is_short))
        .bind(("multi_voice_enabled", multi_voice_enabled))
        .bind(("voice_roles", voice_roles_json))
        .await
        .map_err(|e| Error::Database(format!("create audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("create audiobook: {e}")))?;

    // Persist cover bytes if the client included them. Failure here only
    // logs — it must not block outline generation.
    if let Some(b64) = body.cover_image_base64.as_deref() {
        if let Err(e) = persist_cover(&state, &id, b64).await {
            warn!(error = %e, audiobook_id = id, "create: cover persist failed");
        }
    }

    outline_gen::run(
        &state,
        &user.id,
        &id,
        &body.topic,
        body.length,
        if genre.is_empty() { "any" } else { &genre },
        &language,
        is_short,
    )
    .await?;

    // Auto-pipeline: kick off chapter writing immediately after outline if
    // the user requested it. Subsequent steps (TTS, publish) are chained
    // by the job handlers themselves so a refresh / disconnect doesn't
    // break the flow. Cover runs alongside chapters because it only
    // needs topic + genre + style; no point gating it behind chapter text.
    if let Some(pipeline) = &auto_pipeline {
        if pipeline.chapters {
            let req = EnqueueRequest::new(JobKind::Chapters)
                .with_user(user.id.clone())
                .with_audiobook(AudiobookId(id.clone()));
            if let Err(e) = state.jobs().enqueue(req).await {
                warn!(error = %e, audiobook_id = id, "auto-pipeline: enqueue chapters failed");
            }
        }
        // Skip the cover job if the caller already supplied bytes — they
        // pre-generated their preview and we just persisted it.
        if pipeline.cover && body.cover_image_base64.is_none() {
            // Image gen is flakier than text — give it a larger retry
            // budget so a single content-filter false positive or empty
            // upstream payload doesn't kill the cover.
            let req = EnqueueRequest::new(JobKind::Cover)
                .with_user(user.id.clone())
                .with_audiobook(AudiobookId(id.clone()))
                .with_max_attempts(5);
            if let Err(e) = state.jobs().enqueue(req).await {
                warn!(error = %e, audiobook_id = id, "auto-pipeline: enqueue cover failed");
            }
        }
    }

    Ok(Json(load_detail(&state, &id, &user.id, None).await?))
}

/// Convert the user-supplied pipeline into a normalised, validated form.
/// Empty / no-op pipelines collapse to `None` so we don't persist noise.
fn normalise_auto_pipeline(
    req: &AutoPipelineRequest,
    is_short: bool,
) -> Result<Option<AutoPipelineRequest>> {
    // Audio without chapters is impossible — strip it.
    let audio = req.chapters && req.audio;
    // Publish without audio is impossible — strip it.
    let mut publish = if audio { req.publish.clone() } else { None };
    if let Some(p) = &mut publish {
        // Shorts upload as a single vertical video — playlist mode doesn't
        // apply because the whole story fits in one ≤ 90 s clip.
        if is_short {
            p.mode = Some("single".to_string());
        }
        let mode = p.mode.as_deref().unwrap_or("single");
        if !matches!(mode, "single" | "playlist") {
            return Err(Error::Validation("pipeline.publish.mode must be single or playlist".into()));
        }
        let privacy = p.privacy_status.as_deref().unwrap_or("private");
        if !matches!(privacy, "private" | "unlisted" | "public") {
            return Err(Error::Validation(
                "pipeline.publish.privacy_status must be private/unlisted/public".into(),
            ));
        }
    }
    if !req.chapters && !req.cover && !audio && publish.is_none() {
        return Ok(None);
    }
    Ok(Some(AutoPipelineRequest {
        chapters: req.chapters,
        cover: req.cover,
        audio,
        publish,
    }))
}

// -------------------------------------------------------------------------
// GET /audiobook  — list my audiobooks
// -------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/audiobook",
    tag = "audiobook",
    responses(
        (status = 200, description = "All audiobooks owned by the authed user", body = AudiobookList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
) -> ApiResult<Json<AudiobookList>> {
    let rows: Vec<DbAudiobook> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM audiobook WHERE owner = user:`{}` ORDER BY created_at DESC",
            user.id.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("list audiobooks: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list audiobooks (decode): {e}")))?;

    // One extra round-trip pulls every narrated chapter for this user's
    // books so we can sum runtime per audiobook in Rust. Doing it here
    // (rather than per-row) keeps the list query O(1) regardless of
    // library size.
    #[derive(Deserialize)]
    struct DurationRow {
        audiobook: Thing,
        #[serde(default)]
        language: Option<String>,
        duration_ms: i64,
    }
    let duration_rows: Vec<DurationRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT audiobook, language, duration_ms FROM chapter \
             WHERE audiobook IN (SELECT VALUE id FROM audiobook WHERE owner = user:`{}`) \
             AND duration_ms != NONE",
            user.id.0,
        ))
        .await
        .map_err(|e| Error::Database(format!("list audiobooks (durations): {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list audiobooks (durations decode): {e}")))?;

    // (audiobook_id, language) → summed duration. Using BTreeMap for
    // determinism in tests.
    let mut totals: std::collections::BTreeMap<(String, String), u64> =
        std::collections::BTreeMap::new();
    for r in duration_rows {
        let lang = r.language.unwrap_or_else(|| "en".to_string());
        let book_id = r.audiobook.id.to_raw();
        *totals.entry((book_id, lang)).or_insert(0) += r.duration_ms.max(0) as u64;
    }

    let items = rows
        .iter()
        .map(|row| {
            let mut s = row.to_summary()?;
            let primary = row.language.clone().unwrap_or_else(|| "en".to_string());
            s.duration_ms = totals.get(&(s.id.0.clone(), primary)).copied();
            Ok(s)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(AudiobookList { items }))
}

// -------------------------------------------------------------------------
// GET /audiobook/:id
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GetOneQuery {
    /// Optional language filter; defaults to the audiobook's primary
    /// language. Only chapters in this language are returned.
    #[serde(default)]
    pub language: Option<String>,
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}",
    tag = "audiobook",
    params(
        ("id" = String, Path, description = "Audiobook id"),
        ("language" = Option<String>, Query, description = "Language filter (default: audiobook primary)")
    ),
    responses(
        (status = 200, description = "Audiobook + chapters", body = AudiobookDetail),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn get_one(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<GetOneQuery>,
) -> ApiResult<Json<AudiobookDetail>> {
    Ok(Json(
        load_detail(&state, &id, &user.id, q.language.as_deref()).await?,
    ))
}

// -------------------------------------------------------------------------
// PATCH /audiobook/:id  — edit title or genre
// -------------------------------------------------------------------------

#[utoipa::path(
    patch,
    path = "/audiobook/{id}",
    tag = "audiobook",
    params(("id" = String, Path)),
    request_body = UpdateAudiobookRequest,
    responses(
        (status = 200, description = "Updated audiobook", body = AudiobookDetail),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn patch(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    Json(body): Json<UpdateAudiobookRequest>,
) -> ApiResult<Json<AudiobookDetail>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    assert_owner(&state, &id, &user.id).await?;

    if let Some(title) = body.title {
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET title = $t"))
            .bind(("t", title.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch title: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch title: {e}")))?;
    }
    if let Some(genre) = body.genre {
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET genre = $g"))
            .bind(("g", Some(genre.trim().to_string())))
            .await
            .map_err(|e| Error::Database(format!("patch genre: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch genre: {e}")))?;
    }
    if let Some(cat) = body.category {
        // Empty string clears → moves the book back to "Uncategorized".
        let trimmed = cat.trim();
        let value: Option<String> = if trimmed.is_empty() {
            None
        } else {
            assert_category_exists(&state, trimmed).await?;
            Some(trimmed.to_string())
        };
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET category = $c"))
            .bind(("c", value))
            .await
            .map_err(|e| Error::Database(format!("patch category: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch category: {e}")))?;
    }
    if let Some(voice) = body.voice_id {
        let trimmed = voice.trim();
        if trimmed.is_empty() {
            // Empty string clears the override; the TTS layer falls back to
            // `Config.xai_default_voice`.
            state
                .db()
                .inner()
                .query(format!("UPDATE audiobook:`{id}` SET primary_voice = NONE"))
                .await
                .map_err(|e| Error::Database(format!("clear voice: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("clear voice: {e}")))?;
        } else {
            let resolved = assert_voice_enabled(&state, trimmed).await?;
            state
                .db()
                .inner()
                .query(format!(
                    "UPDATE audiobook:`{id}` SET primary_voice = voice:`{resolved}`"
                ))
                .await
                .map_err(|e| Error::Database(format!("patch voice: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("patch voice: {e}")))?;
        }
    }
    if let Some(style) = body.art_style {
        let trimmed = style.trim();
        // Empty string clears the override; the cover generator falls back
        // to its built-in default on the next regeneration.
        let value: Option<String> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET art_style = $s"))
            .bind(("s", value))
            .await
            .map_err(|e| Error::Database(format!("patch art_style: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch art_style: {e}")))?;
    }
    if let Some(llm) = body.cover_llm_id {
        let trimmed = llm.trim();
        let value: Option<String> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET cover_llm_id = $l"))
            .bind(("l", value))
            .await
            .map_err(|e| Error::Database(format!("patch cover_llm_id: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch cover_llm_id: {e}")))?;
    }
    if let Some(count) = body.images_per_paragraph {
        let capped = count.min(3) as i64;
        let value: Option<i64> = if capped == 0 { None } else { Some(capped) };
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE audiobook:`{id}` SET images_per_paragraph = $c"
            ))
            .bind(("c", value))
            .await
            .map_err(|e| Error::Database(format!("patch images_per_paragraph: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch images_per_paragraph: {e}")))?;
    }
    if let Some(short) = body.is_short {
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET is_short = $s"))
            .bind(("s", short))
            .await
            .map_err(|e| Error::Database(format!("patch is_short: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch is_short: {e}")))?;
    }
    if let Some(enabled) = body.multi_voice_enabled {
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE audiobook:`{id}` SET multi_voice_enabled = $v"
            ))
            .bind(("v", enabled))
            .await
            .map_err(|e| Error::Database(format!("patch multi_voice_enabled: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch multi_voice_enabled: {e}")))?;
    }
    if let Some(roles) = body.voice_roles {
        // Empty map clears the override (back to single-voice fallback).
        let value: Option<serde_json::Value> = if roles.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&roles).unwrap_or(serde_json::Value::Null))
        };
        state
            .db()
            .inner()
            .query(format!("UPDATE audiobook:`{id}` SET voice_roles = $r"))
            .bind(("r", value))
            .await
            .map_err(|e| Error::Database(format!("patch voice_roles: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch voice_roles: {e}")))?;
    }
    // Three-state stem_override:
    //   field absent          → body.stem_override = None         → no-op
    //   field present, null   → Some(Value::Null)                 → clear
    //   field present, bool   → Some(Value::Bool(b))              → set
    //   anything else (number, string, object) → 400.
    if let Some(raw) = body.stem_override {
        let new_value: Option<bool> = match raw {
            serde_json::Value::Null => None,
            serde_json::Value::Bool(b) => Some(b),
            other => {
                return Err(Error::Validation(format!(
                    "stem_override must be a bool or null, got {other:?}"
                ))
                .into())
            }
        };
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE audiobook:`{id}` SET stem_override = $v"
            ))
            .bind(("v", new_value))
            .await
            .map_err(|e| Error::Database(format!("patch stem_override: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch stem_override: {e}")))?;
    }
    if let Some(podcast_id) = body.podcast_id {
        let trimmed = podcast_id.trim();
        if trimmed.is_empty() {
            state
                .db()
                .inner()
                .query(format!("UPDATE audiobook:`{id}` SET podcast = NONE"))
                .await
                .map_err(|e| Error::Database(format!("clear podcast: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("clear podcast: {e}")))?;
        } else {
            // Reject ids the user doesn't own — embedding `podcast:<id>`
            // raw would otherwise let cross-user assignments through.
            assert_podcast_owned(&state, trimmed, &user.id).await?;
            state
                .db()
                .inner()
                .query(format!(
                    "UPDATE audiobook:`{id}` SET podcast = podcast:`{trimmed}`"
                ))
                .await
                .map_err(|e| Error::Database(format!("patch podcast: {e}")))?
                .check()
                .map_err(|e| Error::Database(format!("patch podcast: {e}")))?;
        }
    }

    Ok(Json(load_detail(&state, &id, &user.id, None).await?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/cover  — (re)generate cover from topic + genre
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/cover",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Cover regenerated", body = AudiobookDetail),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream image-gen error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_cover(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<AudiobookDetail>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let bytes = crate::generation::cover::generate(
        &state,
        &user.id,
        Some(&id),
        &book.topic,
        book.genre.as_deref(),
        book.art_style.as_deref(),
        book.cover_llm_id.as_deref(),
        book.is_short.unwrap_or(false),
    )
    .await?;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let b64 = B64.encode(&bytes);
    persist_cover(&state, &id, &b64).await?;
    Ok(Json(load_detail(&state, &id, &user.id, None).await?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/translate  — add a translated language version
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TranslateRequest {
    /// Target language code (e.g. `"nl"`).
    #[validate(length(min = 2, max = 8))]
    pub target_language: String,
    /// Optional source language to translate from (defaults to the
    /// audiobook's primary language).
    #[validate(length(min = 2, max = 8))]
    pub source_language: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TranslateResponse {
    /// Id of the background job. Watch its status via the WebSocket or
    /// `GET /audiobook/:id/jobs`. Chapter rows in the target language land
    /// one at a time as the job progresses.
    pub job_id: String,
    pub target_language: String,
    pub source_language: String,
}

#[utoipa::path(
    post,
    path = "/audiobook/{id}/translate",
    tag = "audiobook",
    params(("id" = String, Path)),
    request_body = TranslateRequest,
    responses(
        (status = 202, description = "Translation queued; poll /audiobook/:id/jobs", body = TranslateResponse),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found"),
        (status = 409, description = "A translation to this language is already running")
    ),
    security(("bearer" = []))
)]
pub async fn translate(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    Json(body): Json<TranslateRequest>,
) -> ApiResult<(StatusCode, Json<TranslateResponse>)> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    if !is_supported_language(&body.target_language) {
        return Err(Error::Validation(format!(
            "unsupported language `{}`",
            body.target_language
        ))
        .into());
    }

    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let primary = book
        .language
        .clone()
        .unwrap_or_else(|| "en".to_string());
    let source = body
        .source_language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&primary)
        .to_string();

    if source == body.target_language {
        return Err(Error::Validation(
            "source and target language must differ".into(),
        )
        .into());
    }

    if has_live_translate_job(&state, &id, &body.target_language).await? {
        return Err(Error::Conflict(format!(
            "translation to `{}` already running",
            body.target_language
        ))
        .into());
    }

    let job_id = state
        .jobs()
        .enqueue(
            EnqueueRequest::new(JobKind::Translate)
                .with_user(user.id.clone())
                .with_audiobook(AudiobookId(id.clone()))
                .with_language(body.target_language.clone())
                .with_payload(serde_json::json!({
                    "source_language": source,
                }))
                .with_max_attempts(3),
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(TranslateResponse {
            job_id: job_id.0,
            target_language: body.target_language,
            source_language: source,
        }),
    ))
}

async fn has_live_translate_job(
    state: &AppState,
    audiobook_id: &str,
    target: &str,
) -> Result<bool> {
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM job \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND kind = \"translate\" \
               AND language = $lang \
               AND status IN [\"queued\", \"running\"] \
             GROUP ALL"
        ))
        .bind(("lang", target.to_string()))
        .await
        .map_err(|e| Error::Database(format!("translate live check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("translate live check (decode): {e}")))?;
    Ok(rows.into_iter().next().map(|r| r.count > 0).unwrap_or(false))
}

// -------------------------------------------------------------------------
// DELETE /audiobook/:id
// -------------------------------------------------------------------------

#[utoipa::path(
    delete,
    path = "/audiobook/{id}",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn delete(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    assert_owner(&state, &id, &user.id).await?;
    state
        .db()
        .inner()
        .query(format!(
            "DELETE job WHERE audiobook = audiobook:`{id}`; \
             DELETE chapter WHERE audiobook = audiobook:`{id}`; \
             DELETE audiobook:`{id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("delete audiobook: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("delete audiobook: {e}")))?;

    let dir = state.config().storage_path.join(&id);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            warn!(error = %e, path = ?dir, "delete audiobook: leaving audio dir for GC");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/generate-chapters  — kicks off async chapter writer
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/generate-chapters",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "Accepted, generation started in background"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Already running or not in outline_ready")
    ),
    security(("bearer" = []))
)]
pub async fn generate_chapters(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    IdempotencyKey(idem): IdempotencyKey,
) -> ApiResult<StatusCode> {
    if let Some(cached) = idempotency::lookup(&state, &user.id, idem.as_deref()).await? {
        return Ok(StatusCode::from_u16(cached.status_code)
            .unwrap_or(StatusCode::ACCEPTED));
    }

    let book = load_audiobook(&state, &id).await?;
    if book.owner_id() != user.id {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        }
        .into());
    }
    let status = parse_status(&book.status)?;
    match status {
        AudiobookStatus::OutlineReady | AudiobookStatus::Failed | AudiobookStatus::TextReady => {}
        _ => {
            return Err(Error::Conflict(format!(
                "audiobook is in state {:?}; only outline_ready, text_ready, or failed can be (re)generated",
                status
            ))
            .into())
        }
    }

    if has_live_job(&state, &id, JobKind::Chapters).await? {
        return Err(Error::Conflict("chapters already in flight".into()).into());
    }

    let req = EnqueueRequest::new(JobKind::Chapters)
        .with_user(user.id.clone())
        .with_audiobook(AudiobookId(id.clone()))
        .with_max_attempts(3);
    state.jobs().enqueue(req).await?;

    idempotency::record(
        &state,
        &user.id,
        idem.as_deref(),
        "POST",
        &format!("/audiobook/{id}/generate-chapters"),
        StatusCode::ACCEPTED.as_u16(),
        "",
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/generate-audio  — TTS for every chapter
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/generate-audio",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "Accepted, TTS started in background"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Chapters not ready to narrate")
    ),
    security(("bearer" = []))
)]
pub async fn generate_audio(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<GetOneQuery>,
    IdempotencyKey(idem): IdempotencyKey,
) -> ApiResult<StatusCode> {
    if let Some(cached) = idempotency::lookup(&state, &user.id, idem.as_deref()).await? {
        return Ok(StatusCode::from_u16(cached.status_code)
            .unwrap_or(StatusCode::ACCEPTED));
    }

    let book = load_audiobook(&state, &id).await?;
    if book.owner_id() != user.id {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        }
        .into());
    }
    let status = parse_status(&book.status)?;
    match status {
        AudiobookStatus::TextReady | AudiobookStatus::AudioReady | AudiobookStatus::Failed => {}
        _ => {
            return Err(Error::Conflict(format!(
                "audiobook is in state {:?}; chapter text must be ready before TTS",
                status
            ))
            .into())
        }
    }

    let language = q
        .language
        .unwrap_or_else(|| book.language.clone().unwrap_or_else(|| "en".to_string()));
    if !is_supported_language(&language) {
        return Err(Error::Validation(format!(
            "unsupported language `{language}`"
        ))
        .into());
    }

    if has_live_job(&state, &id, JobKind::Tts).await? {
        return Err(Error::Conflict("tts already in flight".into()).into());
    }

    let req = EnqueueRequest::new(JobKind::Tts)
        .with_user(user.id.clone())
        .with_audiobook(AudiobookId(id.clone()))
        .with_language(language)
        .with_max_attempts(3);
    state.jobs().enqueue(req).await?;

    idempotency::record(
        &state,
        &user.id,
        idem.as_deref(),
        "POST",
        &format!("/audiobook/{id}/generate-audio"),
        StatusCode::ACCEPTED.as_u16(),
        "",
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/animate  — render animated companion video per chapter
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AnimateQuery {
    /// Audiobook language version to animate. Defaults to the
    /// audiobook's primary language.
    #[serde(default)]
    pub language: Option<String>,
    /// Theme preset for the renderer. One of `library` (default),
    /// `parchment`, or `minimal`. Unknown presets 400.
    #[serde(default)]
    pub theme: Option<String>,
}

/// Kick off the animation pipeline for one language: a parent
/// `Animate` job that fans out one `AnimateChapter` per chapter.
/// Output lands at `<storage>/<audiobook>/<language>/ch-<n>.video.mp4`,
/// ready for the YouTube publisher to mux in (Phase D).
///
/// Phase A: gated on `audio_ready` so we never animate against a
/// missing WAV.
#[utoipa::path(
    post,
    path = "/audiobook/{id}/animate",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 202, description = "Accepted, animation queued in background"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Audio not ready or animation already in flight")
    ),
    security(("bearer" = []))
)]
pub async fn animate(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<AnimateQuery>,
    IdempotencyKey(idem): IdempotencyKey,
) -> ApiResult<StatusCode> {
    if let Some(cached) = idempotency::lookup(&state, &user.id, idem.as_deref()).await? {
        return Ok(StatusCode::from_u16(cached.status_code)
            .unwrap_or(StatusCode::ACCEPTED));
    }

    let book = load_audiobook(&state, &id).await?;
    if book.owner_id() != user.id {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        }
        .into());
    }
    let status = parse_status(&book.status)?;
    if !matches!(status, AudiobookStatus::AudioReady) {
        return Err(Error::Conflict(format!(
            "audiobook is in state {:?}; narrate first (audio_ready required)",
            status
        ))
        .into());
    }

    let language = q
        .language
        .unwrap_or_else(|| book.language.clone().unwrap_or_else(|| "en".to_string()));
    if !is_supported_language(&language) {
        return Err(Error::Validation(format!(
            "unsupported language `{language}`"
        ))
        .into());
    }

    if has_live_job(&state, &id, JobKind::Animate).await? {
        return Err(Error::Conflict("animation already in flight".into()).into());
    }

    // Validate the theme preset before enqueueing — saves the worker a
    // round trip and gives the UI a clear 400 instead of a Phase-A
    // fallback render.
    let theme = q
        .theme
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(t) = theme.as_deref() {
        if !matches!(t, "library" | "parchment" | "minimal") {
            return Err(Error::Validation(format!(
                "unsupported theme `{t}` (expected library, parchment, or minimal)"
            ))
            .into());
        }
    }

    let mut req = EnqueueRequest::new(JobKind::Animate)
        .with_user(user.id.clone())
        .with_audiobook(AudiobookId(id.clone()))
        .with_language(language)
        // Parent is just coordination; only the children do real work.
        .with_max_attempts(2);
    if let Some(t) = theme {
        req = req.with_payload(serde_json::json!({ "theme": t }));
    }
    state.jobs().enqueue(req).await?;

    idempotency::record(
        &state,
        &user.id,
        idem.as_deref(),
        "POST",
        &format!("/audiobook/{id}/animate"),
        StatusCode::ACCEPTED.as_u16(),
        "",
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/animate  — re-render one chapter only
// -------------------------------------------------------------------------

/// Re-render a single chapter's animated MP4. Mirrors
/// `POST /audiobook/:id/animate` but skips the parent fan-out: a
/// single `AnimateChapter` job runs end-to-end against the supplied
/// chapter number, with the same theme + language validation as the
/// full-book endpoint.
///
/// Importantly, this **invalidates the F.1e spec-hash cache** for the
/// chapter — without that, the cache hit would short-circuit the
/// render and the user wouldn't see any change. We delete both the
/// `<mp4>.hash` sidecar and the `<mp4>` itself so the frontend's
/// inline `<video>` 404s mid-rerender (signalling "regenerating")
/// instead of showing a stale frame.
#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/animate",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 202, description = "Accepted, chapter re-render queued"),
        (status = 400, description = "Unsupported language or theme"),
        (status = 404, description = "Audiobook or chapter not found"),
        (status = 409, description = "Audio not ready or chapter render already in flight")
    ),
    security(("bearer" = []))
)]
pub async fn animate_chapter(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
    axum::extract::Query(q): axum::extract::Query<AnimateQuery>,
    IdempotencyKey(idem): IdempotencyKey,
) -> ApiResult<StatusCode> {
    if let Some(cached) = idempotency::lookup(&state, &user.id, idem.as_deref()).await? {
        return Ok(StatusCode::from_u16(cached.status_code).unwrap_or(StatusCode::ACCEPTED));
    }

    let book = load_audiobook(&state, &id).await?;
    if book.owner_id() != user.id {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        }
        .into());
    }
    let status = parse_status(&book.status)?;
    if !matches!(status, AudiobookStatus::AudioReady) {
        return Err(Error::Conflict(format!(
            "audiobook is in state {:?}; narrate first (audio_ready required)",
            status
        ))
        .into());
    }

    let language = q
        .language
        .unwrap_or_else(|| book.language.clone().unwrap_or_else(|| "en".to_string()));
    if !is_supported_language(&language) {
        return Err(Error::Validation(format!(
            "unsupported language `{language}`"
        ))
        .into());
    }

    // Ensure the chapter actually exists in the requested language —
    // otherwise the worker would 404 internally and surface a less
    // helpful "Fatal" status to the UI. We `count()` rather than
    // selecting `id` because SurrealDB returns a record id as a
    // `Thing` enum, which doesn't decode into `serde_json::Value`.
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM chapter \
             WHERE audiobook = audiobook:`{id}` \
               AND number = $n AND language = $lang \
             GROUP ALL"
        ))
        .bind(("n", n as i64))
        .bind(("lang", language.clone()))
        .await
        .map_err(|e| Error::Database(format!("animate_chapter chapter check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("animate_chapter chapter check (decode): {e}")))?;
    let chapter_count = rows.first().map(|r| r.count).unwrap_or(0);
    if chapter_count == 0 {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}/chapter:{n}/{language}"),
        }
        .into());
    }

    if has_live_animate_chapter(&state, &id, n, &language).await? {
        return Err(Error::Conflict(format!(
            "chapter {n} animation already in flight"
        ))
        .into());
    }

    let theme = q
        .theme
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(t) = theme.as_deref() {
        if !matches!(t, "library" | "parchment" | "minimal") {
            return Err(Error::Validation(format!(
                "unsupported theme `{t}` (expected library, parchment, or minimal)"
            ))
            .into());
        }
    }

    // Bust the F.1e cache so the worker's hash check doesn't short-
    // circuit the render. Best-effort — a missing hash sidecar means
    // the render proceeds anyway, which is exactly what we want.
    bust_chapter_cache(&state, &id, &language, n);

    let mut req = EnqueueRequest::new(JobKind::AnimateChapter)
        .with_user(user.id.clone())
        .with_audiobook(AudiobookId(id.clone()))
        .with_chapter(n)
        .with_language(language)
        .with_max_attempts(2);
    if let Some(t) = theme {
        req = req.with_payload(serde_json::json!({ "theme": t }));
    }
    state.jobs().enqueue(req).await?;

    idempotency::record(
        &state,
        &user.id,
        idem.as_deref(),
        "POST",
        &format!("/audiobook/{id}/chapter/{n}/animate"),
        StatusCode::ACCEPTED.as_u16(),
        "",
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Check whether *this specific chapter+language* has an in-flight
/// `AnimateChapter` job. The full-book live-job check on
/// `JobKind::Animate` doesn't catch a parentless re-render because
/// the new endpoint enqueues `AnimateChapter` directly without a
/// parent.
async fn has_live_animate_chapter(
    state: &AppState,
    audiobook_id: &str,
    chapter_number: u32,
    language: &str,
) -> Result<bool> {
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM job \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND kind = $kind \
               AND chapter_number = $n \
               AND language = $lang \
               AND status IN [\"queued\", \"running\"] \
             GROUP ALL"
        ))
        .bind(("kind", JobKind::AnimateChapter.as_str().to_string()))
        .bind(("n", chapter_number as i64))
        .bind(("lang", language.to_string()))
        .await
        .map_err(|e| Error::Database(format!("animate_chapter live check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("animate_chapter live check (decode): {e}")))?;
    Ok(rows.first().map(|r| r.count).unwrap_or(0) > 0)
}

/// Delete the rendered MP4 + the F.1e hash sidecar for the given
/// chapter so the next render produces a fresh artefact. Best
/// effort — missing files are normal (chapter never rendered) and
/// not an error.
fn bust_chapter_cache(state: &AppState, audiobook_id: &str, language: &str, chapter_number: u32) {
    let storage = match std::fs::canonicalize(&state.config().storage_path) {
        Ok(p) => p,
        Err(_) => return,
    };
    let mp4 = crate::animation::planner::output_mp4_path(
        &storage,
        audiobook_id,
        language,
        chapter_number,
    );
    let hash = crate::animation::cache::cache_path(&mp4);
    let _ = std::fs::remove_file(&mp4);
    let _ = std::fs::remove_file(&hash);
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/cancel-pipeline  — abort everything in flight
// -------------------------------------------------------------------------

/// Cancel every non-terminal job for this audiobook and clear the
/// auto-pipeline column so chained handlers don't fan out further steps.
///
/// In-flight jobs keep running to the end of their current chunk — the
/// repo's terminal writes (`mark_completed` / `mark_failed`) are gated on
/// `status = "running"`, so flipping the row to `dead` first makes those
/// writes no-ops and the cancel sticks. Queued and throttled rows stop
/// being eligible for pickup as soon as the UPDATE lands.
#[utoipa::path(
    post,
    path = "/audiobook/{id}/cancel-pipeline",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "All in-flight jobs cancelled"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer" = []))
)]
pub async fn cancel_pipeline(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    assert_owner(&state, &id, &user.id).await?;

    // Mark everything in flight as dead. SurrealDB serialises writes
    // per record, so a concurrent worker terminal-write either lands
    // first (job completes naturally) or sees status != "running" and
    // becomes a no-op.
    state
        .db()
        .inner()
        .query(format!(
            r#"UPDATE job SET
                status = "dead",
                finished_at = time::now(),
                updated_at = time::now(),
                last_error = "cancelled by user",
                worker_id = NONE
              WHERE audiobook = audiobook:`{id}`
                AND status IN ["queued", "running", "throttled"]"#
        ))
        .await
        .map_err(|e| Error::Database(format!("cancel pipeline: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("cancel pipeline: {e}")))?;

    // Drop the auto_pipeline so post-success hooks (chapters → narration,
    // narration → publish) don't fire after the cancel lands.
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{id}` SET auto_pipeline = NONE"
        ))
        .await
        .map_err(|e| Error::Database(format!("cancel pipeline (clear auto): {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("cancel pipeline (clear auto): {e}")))?;

    // Push a fresh snapshot so subscribers re-render without waiting for
    // the next worker progress tick.
    let jobs = state.jobs().list_for_audiobook(&id).await?;
    let snapshots = jobs
        .into_iter()
        .map(|j| listenai_jobs::hub::JobSnapshot {
            id: j.id,
            kind: j.kind.as_str().to_string(),
            status: j.status.as_str().to_string(),
            progress_pct: j.progress_pct,
            attempts: j.attempts,
            chapter_number: j.chapter_number,
            last_error: j.last_error,
        })
        .collect();
    state
        .hub()
        .publish(
            &id,
            listenai_jobs::hub::ProgressEvent::Snapshot {
                audiobook_id: id.clone(),
                jobs: snapshots,
                at: Utc::now(),
            },
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Returns true iff a non-terminal job of `kind` exists for this audiobook.
/// Used to block double-submits while an identical job is still queued or
/// running.
async fn has_live_job(state: &AppState, audiobook_id: &str, kind: JobKind) -> Result<bool> {
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let rows: Vec<CountRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT count() AS count FROM job \
             WHERE audiobook = audiobook:`{audiobook_id}` \
               AND kind = $kind \
               AND status IN [\"queued\", \"running\"] \
             GROUP ALL"
        ))
        .bind(("kind", kind.as_str().to_string()))
        .await
        .map_err(|e| Error::Database(format!("live job check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("live job check (decode): {e}")))?;
    Ok(rows.first().map(|r| r.count).unwrap_or(0) > 0)
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/regenerate-audio  — single-chapter sync
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/regenerate-audio",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Regenerated chapter audio", body = ChapterSummary),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream TTS error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_chapter_audio(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let lang = book.language.clone().unwrap_or_else(|| "en".to_string());
    audio_gen::run_one_by_number(&state, &user.id, &id, n as i64, &lang).await?;
    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/art",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Generated chapter artwork", body = ChapterSummary),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream image-gen error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_chapter_art(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let chapter = load_chapter_by_number(&state, &id, n as i64).await?;
    let bytes = crate::generation::cover::generate_chapter_art(
        &state,
        &user.id,
        &id,
        &book.title,
        &book.topic,
        book.genre.as_deref(),
        book.art_style.as_deref(),
        book.cover_llm_id.as_deref(),
        n,
        &chapter.title,
        chapter.synopsis.as_deref(),
        chapter.body_md.as_deref(),
        book.is_short.unwrap_or(false),
    )
    .await?;
    persist_chapter_art(&state, &id, &chapter.id.id.to_raw(), n, &bytes).await?;
    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/classify-visuals  — backfill visual_kind
// -------------------------------------------------------------------------

/// Tiny helper: keep the merged paragraph list together with the
/// labelled-paragraph count so both the backfill and re-classify
/// branches can return the same shape to the persistence step.
fn merged_with_visual_count(
    merged: Vec<serde_json::Value>,
    labelled: usize,
) -> (Vec<serde_json::Value>, usize) {
    (merged, labelled)
}

/// Re-run the per-paragraph visual classifier (Phase G.2's
/// `paragraphs::extract_visual_kinds`) against an existing chapter's
/// paragraphs without rewriting the chapter body.
///
/// Why this exists: STEM detection runs at outline time and the
/// classifier fires from `chapter_paragraphs` only when the book is
/// STEM at that moment. Books generated before STEM was toggled on
/// (or before Phase G shipped) have empty `visual_kind` columns,
/// which makes `segments::has_diagram_scenes(spec)` return false and
/// the publisher routes around Manim. This endpoint backfills the
/// labels so the next animate render can pick them up.
///
/// Operates on the primary language's chapter only — translations
/// share the primary's paragraph metadata.
#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/classify-visuals",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Classifier ran; chapter summary returned with updated paragraph badges", body = ChapterSummary),
        (status = 400, description = "Book is not STEM (effective is_stem=false); refused"),
        (status = 404, description = "Audiobook or chapter not found"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn classify_chapter_visuals(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let summary = book.to_summary()?;
    if !summary.is_stem {
        return Err(Error::Validation(
            "book is not STEM (set stem_override=true or wait for the LLM to detect it) — \
             classifier refused"
                .into(),
        )
        .into());
    }

    let chapter = load_chapter_by_number(&state, &id, n as i64).await?;
    let paragraphs_json = chapter.paragraphs.clone().unwrap_or_default();
    let chapter_id = chapter.id.id.to_raw();

    let (paragraphs, scenes, updated) = if paragraphs_json.is_empty() {
        // Backfill path: this chapter never had its paragraph
        // metadata written (book pre-dates the paragraphs feature, or
        // the chapter_paragraphs job failed). Run the full
        // split → extract_scenes + extract_visual_kinds → merge
        // pipeline so the row gets populated correctly. Equivalent
        // to `chapter_paragraphs` job for STEM books.
        let body = chapter.body_md.as_deref().unwrap_or("");
        if body.trim().is_empty() {
            return Err(Error::Validation(
                "chapter body is empty — generate it first".into(),
            )
            .into());
        }
        let paragraphs = crate::generation::paragraphs::split(body);
        if paragraphs.is_empty() {
            return Err(Error::Validation(
                "chapter body has no extractable paragraphs (every block is below the minimum length)".into(),
            )
            .into());
        }

        let scenes = crate::generation::paragraphs::extract_scenes(
            &state,
            &user.id,
            &id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &paragraphs,
        )
        .await;

        let visuals = crate::generation::paragraphs::extract_visual_kinds(
            &state,
            &user.id,
            &id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &paragraphs,
        )
        .await;

        // Re-classify path skips the manim code-gen step — the user
        // can trigger that separately via the dedicated regen endpoint
        // (cheaper + clearer than coupling them).
        let codes = std::collections::HashMap::new();
        let merged = crate::generation::paragraphs::merge_for_persist(
            &paragraphs,
            &scenes,
            &visuals,
            &codes,
        );
        (paragraphs.len(), scenes.len(), merged_with_visual_count(merged, visuals.len()))
    } else {
        // Re-classify path: paragraphs already exist; only update
        // visual_kind / visual_params, preserve everything else.
        let mut classifier_input: Vec<crate::generation::paragraphs::Paragraph> =
            Vec::with_capacity(paragraphs_json.len());
        for p in &paragraphs_json {
            let idx = p.index.max(0) as u32;
            let text = p.text.clone();
            if text.trim().is_empty() {
                continue;
            }
            let char_count = p
                .char_count
                .unwrap_or_else(|| text.chars().count() as i64)
                .max(0) as u32;
            classifier_input.push(crate::generation::paragraphs::Paragraph {
                index: idx,
                text,
                char_count,
            });
        }

        let visuals = crate::generation::paragraphs::extract_visual_kinds(
            &state,
            &user.id,
            &id,
            &book.title,
            &book.topic,
            book.genre.as_deref(),
            &chapter.title,
            &classifier_input,
        )
        .await;

        let updated: Vec<serde_json::Value> = paragraphs_json
            .iter()
            .map(|p| {
                let idx = p.index.max(0) as u32;
                let mut entry = serde_json::Map::new();
                entry.insert("index".into(), serde_json::json!(idx));
                entry.insert("text".into(), serde_json::json!(p.text));
                let cc = p
                    .char_count
                    .unwrap_or_else(|| p.text.chars().count() as i64);
                entry.insert("char_count".into(), serde_json::json!(cc));
                entry.insert(
                    "scene_description".into(),
                    serde_json::json!(p.scene_description.clone()),
                );
                entry.insert(
                    "image_paths".into(),
                    serde_json::json!(p.image_paths.clone()),
                );
                if let Some(v) = visuals.get(&idx) {
                    entry.insert(
                        "visual_kind".into(),
                        serde_json::json!(v.visual_kind.clone()),
                    );
                    entry.insert("visual_params".into(), v.visual_params.clone());
                    // Phase H — preserve any existing `manim_code`
                    // when the classifier still considers this
                    // paragraph custom_manim. If the kind changed to
                    // anything else, the old code is irrelevant and
                    // we drop it on the floor (re-classifying is a
                    // full overwrite of the diagram label, but the
                    // code is keyed *to* that label so we'd be
                    // shipping mismatched data otherwise).
                    if v.visual_kind == "custom_manim" {
                        if let Some(prev) = p.manim_code.as_deref() {
                            if !prev.trim().is_empty() {
                                entry.insert(
                                    "manim_code".into(),
                                    serde_json::json!(prev),
                                );
                            }
                        }
                    }
                }
                // If the classifier returned nothing for this paragraph we
                // intentionally drop any prior label — re-classifying is a
                // full overwrite, not an append.
                serde_json::Value::Object(entry)
            })
            .collect();
        (paragraphs_json.len(), 0, merged_with_visual_count(updated, visuals.len()))
    };

    let labelled = updated.1;
    crate::generation::paragraphs::persist(&state, &chapter_id, updated.0).await?;

    tracing::info!(
        audiobook = %id,
        chapter = n,
        paragraphs = paragraphs,
        scenes_added = scenes,
        labelled,
        "classify_chapter_visuals: classifier complete"
    );

    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/regenerate-manim-code  (Phase H)
// -------------------------------------------------------------------------

/// Re-run the bespoke Manim code-gen LLM on every paragraph in this
/// chapter that's currently labelled `custom_manim`. Used both as a
/// backfill (books generated before Phase H landed) and a
/// "regenerate this chapter's diagrams with a different model" knob
/// after the user changes the `LlmRole::ManimCode` assignment in the
/// admin UI.
///
/// Returns 400 when the chapter has no `custom_manim` paragraphs to
/// regenerate (the response would otherwise silently no-op, which is
/// confusing UX). The frontend hides the button in that case.
#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/regenerate-manim-code",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Code-gen ran; chapter summary returned", body = ChapterSummary),
        (status = 400, description = "Chapter has no custom_manim paragraphs"),
        (status = 404, description = "Audiobook or chapter not found"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_chapter_manim_code(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    let book = load_audiobook(&state, &id).await?;
    let chapter = load_chapter_by_number(&state, &id, n as i64).await?;
    let chapter_id = chapter.id.id.to_raw();
    let paragraphs_json = chapter.paragraphs.clone().unwrap_or_default();

    // Collect the paragraphs the classifier marked custom_manim. The
    // ManimCode LLM only ever runs against these — every other
    // visual_kind is template-driven and the code-gen would just
    // waste tokens.
    let custom_indices: Vec<u32> = paragraphs_json
        .iter()
        .filter(|p| p.visual_kind.as_deref() == Some("custom_manim"))
        .map(|p| p.index.max(0) as u32)
        .collect();

    if custom_indices.is_empty() {
        return Err(Error::Validation(
            "no custom_manim paragraphs in this chapter — \
             classify diagrams first or pick custom_manim in the override"
                .into(),
        )
        .into());
    }

    let custom_paragraphs: Vec<crate::generation::manim_code::CustomParagraph> =
        paragraphs_json
            .iter()
            .filter(|p| p.visual_kind.as_deref() == Some("custom_manim"))
            .map(|p| crate::generation::manim_code::CustomParagraph {
                index: p.index.max(0) as u32,
                text: p.text.as_str(),
                // No per-paragraph audio plan here either; the
                // publisher floors `run_seconds` at MIN_RUN_SECONDS
                // anyway (Python side, _base.py).
                run_seconds: 8.0,
            })
            .collect();

    let codes = crate::generation::manim_code::generate_manim_code(
        &state,
        &user.id,
        &id,
        &book.title,
        &book.topic,
        book.genre.as_deref(),
        &chapter.title,
        // Theme is per-publication, not per-paragraph; we use the
        // library default here so the generated code matches what
        // every other animation path uses today.
        "library",
        &custom_paragraphs,
    )
    .await;

    // Stitch the codes back into the existing paragraph list.
    // Paragraphs we didn't touch keep their prior fields verbatim;
    // the custom_manim ones get a fresh `manim_code`. Empty/whitespace
    // generations are dropped — the publisher will fall back to prose.
    let updated: Vec<serde_json::Value> = paragraphs_json
        .iter()
        .map(|p| {
            let idx = p.index.max(0) as u32;
            let mut entry = serde_json::Map::new();
            entry.insert("index".into(), serde_json::json!(idx));
            entry.insert("text".into(), serde_json::json!(p.text));
            let cc = p
                .char_count
                .unwrap_or_else(|| p.text.chars().count() as i64);
            entry.insert("char_count".into(), serde_json::json!(cc));
            entry.insert(
                "scene_description".into(),
                serde_json::json!(p.scene_description.clone()),
            );
            entry.insert(
                "image_paths".into(),
                serde_json::json!(p.image_paths.clone()),
            );
            if let Some(kind) = p.visual_kind.as_deref() {
                entry.insert("visual_kind".into(), serde_json::json!(kind));
                if let Some(params) = p.visual_params.as_ref() {
                    entry.insert("visual_params".into(), params.clone());
                }
            }
            // Apply the freshly-generated code (when any) and drop
            // empty results. Paragraphs that aren't custom_manim
            // never had code in the first place; no-op for them.
            if let Some(code) = codes.get(&idx) {
                if !code.code.trim().is_empty() {
                    entry.insert(
                        "manim_code".into(),
                        serde_json::json!(code.code),
                    );
                }
            } else if let Some(prev) = p.manim_code.as_deref() {
                // LLM didn't produce anything fresh for this index;
                // preserve the prior code so a partial re-gen
                // failure doesn't wipe working diagrams.
                if !prev.trim().is_empty() {
                    entry.insert("manim_code".into(), serde_json::json!(prev));
                }
            }
            serde_json::Value::Object(entry)
        })
        .collect();

    crate::generation::paragraphs::persist(&state, &chapter_id, updated).await?;

    tracing::info!(
        audiobook = %id,
        chapter = n,
        custom_paragraphs = custom_indices.len(),
        generated = codes.len(),
        "regenerate_chapter_manim_code: complete"
    );

    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/test-manim-llm  — owner-scoped dry-run
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct TestChapterManimLlmRequest {
    /// LLM record id (e.g. `claude_sonnet_4_6`). Backend resolves it
    /// to find provider + upstream model slug. Doesn't have to be
    /// tagged `default_for: ["manim_code"]` — the whole point is to
    /// audition models the user hasn't committed to yet.
    #[validate(length(min = 1, max = 120))]
    pub llm_id: String,
    /// Optional paragraph index to test against. Defaults to the
    /// first `custom_manim` paragraph if any, otherwise paragraph 0.
    pub paragraph_index: Option<u32>,
    /// Theme name passed to the prompt. Defaults to `library` to
    /// match the production code-gen path.
    #[validate(length(max = 40))]
    pub theme: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestChapterManimLlmResponse {
    /// Display name of the LLM that ran (e.g. "Claude Sonnet 4.6").
    pub llm_name: String,
    /// Upstream model slug (e.g. "anthropic/claude-sonnet-4.6").
    pub model_id: String,
    /// Rendered prompt body sent to the LLM, with all `{{markers}}`
    /// already substituted. Useful for spotting prompt bugs without
    /// digging into the template editor.
    pub prompt: String,
    /// Raw response string from the LLM. The production path expects
    /// `{"summary":"…","code":"…"}`; surface whatever came back so the
    /// user can see when a model emits non-JSON or refuses.
    pub response: String,
    /// USD cost. Mirrors the `generation_event` cost rule: `usage.cost`
    /// when the upstream populated it, else token-pricing × usage.
    /// Always `0.0` for mocked calls.
    pub cost_usd: f64,
    /// Wall-clock time the `chat()` call took (request → response
    /// fully decoded).
    pub elapsed_ms: u64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// True when the provider for this LLM has no API key configured;
    /// the response is fabricated locally.
    pub mocked: bool,
    /// Index of the paragraph the test ran against.
    pub paragraph_index: u32,
    /// First 200 chars of the paragraph text. Saves the UI a second
    /// round-trip just to label the dialog.
    pub paragraph_preview: String,
}

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/test-manim-llm",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    request_body = TestChapterManimLlmRequest,
    responses(
        (status = 200, description = "LLM ran; prompt/response/metrics returned", body = TestChapterManimLlmResponse),
        (status = 400, description = "No paragraphs to test against, or unknown llm_id"),
        (status = 404, description = "Audiobook or chapter not found"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn test_chapter_manim_llm(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
    Json(body): Json<TestChapterManimLlmRequest>,
) -> ApiResult<Json<TestChapterManimLlmResponse>> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    assert_owner(&state, &id, &user.id).await?;

    let book = load_audiobook(&state, &id).await?;
    let chapter = load_chapter_by_number(&state, &id, n as i64).await?;
    let paragraphs_json = chapter.paragraphs.clone().unwrap_or_default();
    if paragraphs_json.is_empty() {
        return Err(Error::Validation(
            "chapter has no paragraphs — split the body first (the chapter_paragraphs job)".into(),
        )
        .into());
    }

    // Paragraph selection: explicit > first custom_manim > first row.
    // We never silently fall through when the explicit index is wrong —
    // that'd produce a confusing "ran the wrong paragraph" result.
    let pick = match body.paragraph_index {
        Some(idx) => paragraphs_json
            .iter()
            .find(|p| p.index.max(0) as u32 == idx)
            .cloned()
            .ok_or_else(|| {
                Error::Validation(format!("paragraph {idx} not found in chapter {n}"))
            })?,
        None => paragraphs_json
            .iter()
            .find(|p| p.visual_kind.as_deref() == Some("custom_manim"))
            .cloned()
            .unwrap_or_else(|| paragraphs_json[0].clone()),
    };

    // Resolve LLM by id. Inline the query so we don't have to expose
    // admin's `load_llm` to non-admin handlers.
    #[derive(Deserialize)]
    struct LlmRow {
        name: String,
        provider: String,
        model_id: String,
        #[serde(default)]
        cost_prompt_per_1k: f64,
        #[serde(default)]
        cost_completion_per_1k: f64,
    }
    let llm_id = body.llm_id.trim();
    if !llm_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(Error::Validation("invalid llm_id charset".into()).into());
    }
    let llm: LlmRow = state
        .db()
        .inner()
        .query(format!(
            "SELECT name, provider, model_id, cost_prompt_per_1k, cost_completion_per_1k \
             FROM llm:`{llm_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("test-manim-llm load llm: {e}")))?
        .take::<Vec<LlmRow>>(0)
        .map_err(|e| Error::Database(format!("test-manim-llm decode llm: {e}")))?
        .into_iter()
        .next()
        .ok_or_else(|| Error::NotFound {
            resource: format!("llm:{llm_id}"),
        })?;

    // Render the manim_code prompt with the same vars the production
    // code-gen path uses; only `theme` and `run_seconds` differ in
    // that we don't have a per-paragraph audio plan here, so we use
    // the same defaults the regen handler uses.
    let theme = body
        .theme
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("library");
    let mut vars: std::collections::HashMap<&str, String> =
        std::collections::HashMap::new();
    vars.insert("book_title", book.title.clone());
    vars.insert("book_topic", book.topic.clone());
    vars.insert("genre", book.genre.clone().unwrap_or_else(|| "any".into()));
    vars.insert("chapter_title", chapter.title.clone());
    vars.insert("theme", theme.to_string());
    vars.insert("run_seconds", "8.0".into());
    vars.insert("paragraph_text", pick.text.clone());
    let rendered = crate::generation::prompts::render(
        &state,
        listenai_core::domain::prompt::PromptRole::ManimCode,
        &vars,
    )
    .await?;

    // Call the LLM directly using the resolved model_id + provider.
    // Bypasses `pick_llm_for_role` so the user can audition any
    // enabled model, not just the one tagged `default_for: manim_code`.
    use crate::llm::{ChatMessage, ChatRequest};
    let req = ChatRequest {
        model: llm.model_id.clone(),
        messages: vec![
            ChatMessage::system(
                "You write Manim Community Edition code for one diagram. \
                 Reply with strict JSON: {\"summary\": \"...\", \"code\": \"...\"}. \
                 No markdown fences, no prose outside the JSON.",
            ),
            ChatMessage::user(rendered.body.clone()),
        ],
        temperature: Some(0.4),
        max_tokens: Some(4_000),
        json_mode: Some(true),
        modalities: None,
        provider: Some(llm.provider.clone()),
    };
    let started = std::time::Instant::now();
    let resp = state.llm().chat(&req).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    // Cost — same priority order as `outline::log_generation_event`,
    // minus the `mocked` short-circuit (already 0).
    let cost_usd = if resp.mocked {
        0.0
    } else if resp.usage.cost > 0.0 {
        resp.usage.cost
    } else {
        let pt = resp.usage.prompt_tokens as f64;
        let ct = resp.usage.completion_tokens as f64;
        (pt / 1000.0) * llm.cost_prompt_per_1k
            + (ct / 1000.0) * llm.cost_completion_per_1k
    };

    let preview: String = pick.text.chars().take(200).collect();

    Ok(Json(TestChapterManimLlmResponse {
        llm_name: llm.name,
        model_id: llm.model_id,
        prompt: rendered.body,
        response: resp.content,
        cost_usd,
        elapsed_ms,
        prompt_tokens: resp.usage.prompt_tokens,
        completion_tokens: resp.usage.completion_tokens,
        mocked: resp.mocked,
        paragraph_index: pick.index.max(0) as u32,
        paragraph_preview: preview,
    }))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/test-manim-render
//   — render Manim code straight into a throwaway MP4 so the test
//     dialog can preview what the audition LLM produced. No DB writes.
// -------------------------------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RenderTestManimRequest {
    /// Raw Python source for one `class Scene(TemplateScene): …`. Same
    /// shape the production `manim_code` path persists; the sidecar
    /// AST-screens before exec'ing.
    #[validate(length(min = 1, max = 200_000))]
    pub code: String,
    /// Target run length. Defaults to 8 s — matches the placeholder
    /// the test-manim-llm prompt uses, and is well within the
    /// MIN_RUN_SECONDS floor the sidecar enforces.
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderTestManimResponse {
    /// Opaque id (UUID) the frontend uses with the matching `GET
    /// /audiobook/:id/test-manim/:test_id` stream endpoint to fetch
    /// the rendered MP4.
    pub test_id: String,
    /// Wall-clock time the Manim sidecar took (spawn → MP4 done).
    pub elapsed_ms: u64,
}

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/test-manim-render",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    request_body = RenderTestManimRequest,
    responses(
        (status = 200, description = "Rendered MP4 ready; fetch via test-manim/:test_id", body = RenderTestManimResponse),
        (status = 400, description = "Manim sidecar not configured or code rejected"),
        (status = 404, description = "Audiobook not found"),
        (status = 502, description = "Render failed")
    ),
    security(("bearer" = []))
)]
pub async fn render_test_manim(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, _n)): Path<(String, u32)>,
    Json(body): Json<RenderTestManimRequest>,
) -> ApiResult<Json<RenderTestManimResponse>> {
    body.validate().map_err(|e| Error::Validation(e.to_string()))?;
    assert_owner(&state, &id, &user.id).await?;

    let cfg = state.config();
    if cfg.animate_manim_cmd.trim().is_empty() {
        return Err(Error::Validation(
            "animate_manim_cmd is empty — Manim sidecar not configured on this server"
                .into(),
        )
        .into());
    }

    // Per-audiobook test-manim directory under storage. Reusing
    // storage_path keeps these throwaway clips on the same disk as
    // chapter audio/video, so the streaming handler doesn't need
    // a second base.
    let storage = std::fs::canonicalize(&cfg.storage_path).map_err(|e| {
        Error::Other(anyhow::anyhow!(
            "canonicalize storage_path {:?}: {e}",
            cfg.storage_path
        ))
    })?;
    let test_dir = storage.join(&id).join("test-manim");
    std::fs::create_dir_all(&test_dir).map_err(|e| {
        Error::Other(anyhow::anyhow!(
            "create test-manim dir {}: {e}",
            test_dir.display()
        ))
    })?;
    let test_id = uuid::Uuid::new_v4().simple().to_string();
    let output_mp4 = test_dir.join(format!("{test_id}.mp4"));

    // One-shot sidecar pool. Capacity 1 — testing is sequential, the
    // user clicks Run, waits, repeats. Sidecar startup is ~3–5 s
    // (Manim Python imports), which we eat each test; sharing across
    // requests would speed it up but means storing a pool on AppState
    // and tracking shutdown — overkill for an audition feature.
    use crate::animation::manim_sidecar::{
        ManimRendererPool, ManimRequest, ManimSidecarCfg,
    };
    let pool = ManimRendererPool::new(
        ManimSidecarCfg::new(
            cfg.animate_manim_python_bin.clone(),
            std::path::PathBuf::from(&cfg.animate_manim_cmd),
            cfg.animate_manim_ld_preload.clone(),
        ),
        1,
    );

    let duration_ms = body.duration_ms.unwrap_or(8_000).max(1_000);
    let req = ManimRequest::RawScene {
        code: body.code,
        duration_ms,
        output_mp4: output_mp4.clone(),
    };

    let started = std::time::Instant::now();
    let render_result = pool.render(&req).await;
    pool.shutdown().await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if let Err(e) = render_result {
        // Best-effort cleanup so a failed render doesn't leave a
        // 0-byte stub the stream endpoint would happily serve.
        let _ = std::fs::remove_file(&output_mp4);
        return Err(Error::Upstream(format!("manim render failed: {e:?}")).into());
    }

    Ok(Json(RenderTestManimResponse {
        test_id,
        elapsed_ms,
    }))
}

// -------------------------------------------------------------------------
// PATCH /audiobook/:id/chapter/:n
// -------------------------------------------------------------------------

#[utoipa::path(
    patch,
    path = "/audiobook/{id}/chapter/{n}",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    request_body = UpdateChapterRequest,
    responses(
        (status = 200, description = "Updated chapter", body = ChapterSummary),
        (status = 400, description = "Validation failed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn patch_chapter(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
    Json(body): Json<UpdateChapterRequest>,
) -> ApiResult<Json<ChapterSummary>> {
    body.validate()
        .map_err(|e| Error::Validation(e.to_string()))?;
    assert_owner(&state, &id, &user.id).await?;

    let chapter = load_chapter_by_number(&state, &id, n as i64).await?;
    let raw = chapter.id.id.to_raw();

    if let Some(title) = body.title {
        state
            .db()
            .inner()
            .query(format!("UPDATE chapter:`{raw}` SET title = $t"))
            .bind(("t", title.trim().to_string()))
            .await
            .map_err(|e| Error::Database(format!("patch chapter title: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch chapter title: {e}")))?;
    }
    if let Some(syn) = body.synopsis {
        state
            .db()
            .inner()
            .query(format!("UPDATE chapter:`{raw}` SET synopsis = $s"))
            .bind(("s", Some(syn.trim().to_string())))
            .await
            .map_err(|e| Error::Database(format!("patch chapter synopsis: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch chapter synopsis: {e}")))?;
    }
    if let Some(bm) = body.body_md {
        // Body change invalidates the cached voice_segments (the old
        // segmentation no longer matches the prose). Cleared in the
        // same statement so a subsequent multi-voice narration
        // re-runs the extract pass against the fresh body.
        state
            .db()
            .inner()
            .query(format!(
                "UPDATE chapter:`{raw}` SET body_md = $b, status = \"text_ready\", \
                 voice_segments = NONE"
            ))
            .bind(("b", Some(bm)))
            .await
            .map_err(|e| Error::Database(format!("patch chapter body: {e}")))?
            .check()
            .map_err(|e| Error::Database(format!("patch chapter body: {e}")))?;
    }

    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// POST /audiobook/:id/chapter/:n/regenerate  — synchronous single-chapter
// -------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/audiobook/{id}/chapter/{n}/regenerate",
    tag = "audiobook",
    params(("id" = String, Path), ("n" = u32, Path)),
    responses(
        (status = 200, description = "Regenerated chapter", body = ChapterSummary),
        (status = 404, description = "Not found"),
        (status = 502, description = "Upstream LLM error")
    ),
    security(("bearer" = []))
)]
pub async fn regenerate_chapter(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path((id, n)): Path<(String, u32)>,
) -> ApiResult<Json<ChapterSummary>> {
    assert_owner(&state, &id, &user.id).await?;
    chapter_gen::run_one_by_number(&state, &user.id, &id, n as i64).await?;
    let after = load_chapter_by_number(&state, &id, n as i64).await?;
    Ok(Json(after.to_summary()?))
}

// -------------------------------------------------------------------------
// GET /audiobook/:id/costs  — aggregated generation cost
// -------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct CostByRole {
    pub role: String,
    pub count: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd: f64,
    /// LLM record id (`llm:<id>`'s key portion). `None` for events whose
    /// FK couldn't be resolved (notably the `_default_` fallback path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_id: Option<String>,
    /// LLM display name (e.g. "Claude Sonnet 4.6"). For TTS rows, this
    /// holds the voice id parsed from the event's note instead — the
    /// stored `llm` link is a placeholder, not the actual narrator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_name: Option<String>,
    /// Upstream model slug (e.g. "anthropic/claude-sonnet-4.6"). Empty
    /// for TTS rows for the same reason as `llm_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookCostSummary {
    pub audiobook_id: String,
    pub total_cost_usd: f64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub event_count: u32,
    /// Per-role rollup. Now also keyed by LLM, so a role that used two
    /// different models (e.g. an admin swapped the chapter LLM mid-way)
    /// produces two entries with the same `role`.
    pub by_role: Vec<CostByRole>,
}

#[utoipa::path(
    get,
    path = "/audiobook/{id}/costs",
    tag = "audiobook",
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "Cost rollup for the audiobook", body = AudiobookCostSummary),
        (status = 404, description = "Not found")
    ),
    security(("bearer" = []))
)]
pub async fn costs(
    State(state): State<AppState>,
    Authenticated(user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<AudiobookCostSummary>> {
    assert_owner(&state, &id, &user.id).await?;

    #[derive(Debug, Deserialize)]
    struct EventRow {
        role: String,
        prompt_tokens: i64,
        completion_tokens: i64,
        #[serde(default)]
        cost_usd: f64,
        /// FK into `llm`. Always populated by the writers — either a real
        /// row id, the `_default_` placeholder when the role pick fell
        /// through to the env-configured fallback, or the TTS placeholder.
        #[serde(default)]
        llm: Option<Thing>,
        /// TTS rows stash `voice=<id> duration_ms=… chars=…` in here on
        /// success so the admin panel can show the narrator without a
        /// dedicated column. We pull it out below to populate `llm_name`.
        #[serde(default)]
        error: Option<String>,
    }

    let rows: Vec<EventRow> = state
        .db()
        .inner()
        .query(format!(
            "SELECT role, prompt_tokens, completion_tokens, cost_usd, llm, error \
             FROM generation_event WHERE audiobook = audiobook:`{id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("load costs: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load costs (decode): {e}")))?;

    // Group key includes the llm record id so a role that used two
    // different models renders as two rows in the breakdown. `_default_`
    // and missing rows collapse into a single key — the dialog will show
    // them as "fallback" / unnamed.
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Debug, Deserialize)]
    struct LlmMeta {
        id: Thing,
        name: String,
        model_id: String,
    }

    // Distinct llm ids referenced by the events, minus the `_default_`
    // placeholder (no DB row exists for it) and any TTS rows (whose
    // stored llm link is a hardcoded stand-in, not the narrator).
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

    // One round-trip to fetch the display names + upstream model slugs.
    // `record::id(id) INSIDE $ids` avoids string-splicing record-link
    // literals — the ids stay a bound param, which is also cheaper for
    // SurrealDB to validate.
    let mut llm_meta: BTreeMap<String, (String, String)> = BTreeMap::new();
    if !wanted.is_empty() {
        let ids: Vec<String> = wanted.iter().cloned().collect();
        let metas: Vec<LlmMeta> = state
            .db()
            .inner()
            .query(
                "SELECT id, name, model_id FROM llm \
                 WHERE record::id(id) INSIDE $ids",
            )
            .bind(("ids", ids))
            .await
            .map_err(|e| Error::Database(format!("load llm meta: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("load llm meta (decode): {e}")))?;
        for m in metas {
            llm_meta.insert(m.id.id.to_raw(), (m.name, m.model_id));
        }
    }

    // Group by (role, llm_id). The key is what we sort by — alphabetical
    // role keeps callers' UIs deterministic across reloads.
    let mut by_key: BTreeMap<(String, String), CostByRole> = BTreeMap::new();
    let mut total_cost = 0.0;
    let mut total_pt: u64 = 0;
    let mut total_ct: u64 = 0;
    let event_count = rows.len() as u32;
    for r in rows {
        let pt = r.prompt_tokens.max(0) as u64;
        let ct = r.completion_tokens.max(0) as u64;
        total_cost += r.cost_usd;
        total_pt += pt;
        total_ct += ct;

        // Resolve the human-readable model fields. For TTS we ignore the
        // placeholder llm link and parse `voice=<id>` out of the note; for
        // everything else we look up the joined `llm` row, falling back
        // to the raw id when the row was deleted/renamed.
        let (llm_id_opt, llm_name_opt, model_id_opt) = if r.role == "tts" {
            let voice = r
                .error
                .as_deref()
                .and_then(|s| s.split_whitespace().find_map(|t| t.strip_prefix("voice=")))
                .map(str::to_string);
            (None, voice, None)
        } else {
            let raw = r.llm.as_ref().map(|t| t.id.to_raw());
            match raw {
                None => (None, None, None),
                Some(id) if id == "_default_" => (Some(id), None, None),
                Some(id) => match llm_meta.get(&id) {
                    Some((name, model)) => {
                        (Some(id), Some(name.clone()), Some(model.clone()))
                    }
                    None => (Some(id), None, None),
                },
            }
        };

        let key = (r.role.clone(), llm_id_opt.clone().unwrap_or_default());
        let entry = by_key.entry(key).or_insert(CostByRole {
            role: r.role,
            count: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: 0.0,
            llm_id: llm_id_opt,
            llm_name: llm_name_opt,
            model_id: model_id_opt,
        });
        entry.count += 1;
        entry.prompt_tokens += pt;
        entry.completion_tokens += ct;
        entry.cost_usd += r.cost_usd;
    }

    Ok(Json(AudiobookCostSummary {
        audiobook_id: id,
        total_cost_usd: total_cost,
        total_prompt_tokens: total_pt,
        total_completion_tokens: total_ct,
        event_count,
        by_role: by_key.into_values().collect(),
    }))
}

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

async fn load_audiobook(state: &AppState, id: &str) -> Result<DbAudiobook> {
    let rows: Vec<DbAudiobook> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM audiobook:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("load audiobook: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load audiobook (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{id}"),
    })
}

async fn load_detail(
    state: &AppState,
    id: &str,
    user: &UserId,
    language_filter: Option<&str>,
) -> Result<AudiobookDetail> {
    let book = load_audiobook(state, id).await?;
    if book.owner_id() != *user {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        });
    }
    let primary = book
        .language
        .clone()
        .unwrap_or_else(|| "en".to_string());
    let active = language_filter
        .map(str::to_string)
        .unwrap_or_else(|| primary.clone());

    let all_chapters: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM chapter WHERE audiobook = audiobook:`{id}` \
             ORDER BY language ASC, number ASC"
        ))
        .await
        .map_err(|e| Error::Database(format!("load chapters: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapters (decode): {e}")))?;

    // Distinct languages, primary first.
    let mut langs: Vec<String> = all_chapters
        .iter()
        .filter_map(|c| c.language.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !langs.contains(&primary) {
        langs.insert(0, primary.clone());
    } else {
        // Hoist primary to the front for stable UX.
        langs.retain(|l| l != &primary);
        langs.insert(0, primary.clone());
    }

    let chapters: Vec<ChapterSummary> = all_chapters
        .iter()
        .filter(|c| c.language.as_deref().unwrap_or("en") == active)
        .map(DbChapter::to_summary)
        .collect::<Result<Vec<_>>>()?;

    // Sum the *primary* language's chapter durations so the value is
    // stable across language switches in the UI (matches what the list
    // endpoint reports).
    let primary_total: u64 = all_chapters
        .iter()
        .filter(|c| c.language.as_deref().unwrap_or("en") == primary)
        .filter_map(|c| c.duration_ms)
        .map(|d| d.max(0) as u64)
        .sum();

    let mut summary = book.to_summary()?;
    summary.available_languages = langs;
    summary.duration_ms = if primary_total > 0 { Some(primary_total) } else { None };
    Ok(AudiobookDetail { summary, chapters })
}

/// Languages the UI exposes; mirrors the dropdown on the New Audiobook page.
/// Centralising this keeps the validation surface in one place — adding a
/// new language is a single-list edit on either side.
const SUPPORTED_LANGUAGES: &[&str] = &[
    "en", "nl", "fr", "de", "es", "it", "pt", "ru", "zh", "ja", "ko",
];

fn is_supported_language(code: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&code)
}

/// Decode a base64 cover image and write it under
/// `<storage_path>/<audiobook_id>/cover.<ext>`, then update the audiobook's
/// `cover_path` column. The on-disk extension matches the sniffed MIME so
/// the stream handler can serve it with the right Content-Type.
pub(crate) async fn persist_cover(state: &AppState, audiobook_id: &str, b64: &str) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    // Tolerate `data:image/png;base64,...` payloads from less-careful clients.
    let raw = trimmed
        .find(";base64,")
        .map(|i| &trimmed[(i + ";base64,".len())..])
        .unwrap_or(trimmed);
    let bytes = B64
        .decode(raw.as_bytes())
        .map_err(|e| Error::Validation(format!("cover_image_base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Validation("cover_image_base64 is empty".into()));
    }
    let ext = match crate::handlers::cover::detect_mime(&bytes) {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = state.config().storage_path.join(audiobook_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(anyhow::anyhow!("create cover dir {dir:?}: {e}")))?;
    let filename = format!("cover.{ext}");
    let path = dir.join(&filename);
    std::fs::write(&path, &bytes)
        .map_err(|e| Error::Other(anyhow::anyhow!("write cover {path:?}: {e}")))?;

    let rel = format!("{audiobook_id}/{filename}");
    // Bump `updated_at` so the frontend's cache-buster (which keys off it)
    // changes — without this, a regenerated cover stays visually identical
    // because the browser still has the old bytes for the same URL.
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET cover_path = $p, updated_at = time::now()"
        ))
        .bind(("p", rel))
        .await
        .map_err(|e| Error::Database(format!("set cover_path: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set cover_path: {e}")))?;
    Ok(())
}

pub(crate) async fn persist_chapter_art(
    state: &AppState,
    audiobook_id: &str,
    chapter_id: &str,
    chapter_number: u32,
    bytes: &[u8],
) -> Result<()> {
    if bytes.is_empty() {
        return Err(Error::Validation("chapter artwork image is empty".into()));
    }
    let ext = match crate::handlers::cover::detect_mime(bytes) {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = state.config().storage_path.join(audiobook_id).join("chapters");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::Other(anyhow::anyhow!("create chapter art dir {dir:?}: {e}")))?;
    let filename = format!("{chapter_number}-art.{ext}");
    let path = dir.join(&filename);
    std::fs::write(&path, bytes)
        .map_err(|e| Error::Other(anyhow::anyhow!("write chapter art {path:?}: {e}")))?;

    let rel = format!("{audiobook_id}/chapters/{filename}");
    state
        .db()
        .inner()
        .query(format!("UPDATE chapter:`{chapter_id}` SET chapter_art_path = $p"))
        .bind(("p", rel))
        .await
        .map_err(|e| Error::Database(format!("set chapter_art_path: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set chapter_art_path: {e}")))?;
    // Bump the audiobook's `updated_at` too so the frontend cache-buster
    // keyed on it busts the chapter-art URL just like the cover URL.
    bump_audiobook_updated(state, audiobook_id).await.ok();
    Ok(())
}

/// Touches `audiobook.updated_at = now()`. Best-effort: a failure here
/// shouldn't fail the parent regeneration, just means the browser may
/// keep showing the old image until a hard refresh.
async fn bump_audiobook_updated(state: &AppState, audiobook_id: &str) -> Result<()> {
    state
        .db()
        .inner()
        .query(format!(
            "UPDATE audiobook:`{audiobook_id}` SET updated_at = time::now()"
        ))
        .await
        .map_err(|e| Error::Database(format!("bump updated_at: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("bump updated_at: {e}")))?;
    Ok(())
}

/// Persist one paragraph illustration tile. Writes to disk under
/// `<audiobook>/chapters/<n>-p<paragraph>-<ordinal>.<ext>` and updates
/// `chapter.paragraphs[paragraph_index].image_paths[ordinal-1]` in-place.
///
/// The handler is responsible for ensuring the paragraph index is in
/// range — this function will widen the array if a later ordinal lands
/// before earlier ones.
pub(crate) async fn persist_paragraph_image(
    state: &AppState,
    audiobook_id: &str,
    chapter_id: &str,
    chapter_number: u32,
    paragraph_index: u32,
    ordinal: u32,
    bytes: &[u8],
) -> Result<()> {
    if bytes.is_empty() {
        return Err(Error::Validation("paragraph image is empty".into()));
    }
    if ordinal == 0 {
        return Err(Error::Validation("ordinal must be >= 1".into()));
    }
    let ext = match crate::handlers::cover::detect_mime(bytes) {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = state.config().storage_path.join(audiobook_id).join("chapters");
    std::fs::create_dir_all(&dir).map_err(|e| {
        Error::Other(anyhow::anyhow!("create paragraph art dir {dir:?}: {e}"))
    })?;
    let filename = format!("{chapter_number}-p{paragraph_index}-{ordinal}.{ext}");
    let path = dir.join(&filename);
    std::fs::write(&path, bytes)
        .map_err(|e| Error::Other(anyhow::anyhow!("write paragraph image {path:?}: {e}")))?;

    // Load → mutate → write back. The whole `paragraphs` array is
    // FLEXIBLE so we round-trip arbitrary inner keys; we only mutate
    // the target paragraph's `image_paths` slot.
    let rows: Vec<DbChapterParagraphs> = state
        .db()
        .inner()
        .query(format!("SELECT paragraphs FROM chapter:`{chapter_id}`"))
        .await
        .map_err(|e| Error::Database(format!("load paragraphs: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load paragraphs (decode): {e}")))?;
    let mut paragraphs: Vec<serde_json::Value> = rows
        .into_iter()
        .next()
        .and_then(|r| r.paragraphs)
        .unwrap_or_default();

    let target = paragraphs
        .iter_mut()
        .find(|p| {
            p.get("index")
                .and_then(serde_json::Value::as_i64)
                .map(|i| i == paragraph_index as i64)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            Error::Validation(format!(
                "paragraph {paragraph_index} not found on chapter {chapter_number}"
            ))
        })?;
    let obj = target.as_object_mut().ok_or_else(|| {
        Error::Database("paragraph entry is not an object".into())
    })?;
    let entry = obj
        .entry("image_paths".to_string())
        .or_insert_with(|| serde_json::json!([]));
    let arr = entry.as_array_mut().ok_or_else(|| {
        Error::Database("paragraph image_paths is not an array".into())
    })?;
    let slot = (ordinal as usize).saturating_sub(1);
    while arr.len() <= slot {
        arr.push(serde_json::Value::String(String::new()));
    }
    arr[slot] = serde_json::Value::String(format!("{audiobook_id}/chapters/{filename}"));

    state
        .db()
        .inner()
        .query(format!(
            "UPDATE chapter:`{chapter_id}` SET paragraphs = $p"
        ))
        .bind(("p", paragraphs))
        .await
        .map_err(|e| Error::Database(format!("set paragraphs: {e}")))?
        .check()
        .map_err(|e| Error::Database(format!("set paragraphs: {e}")))?;
    bump_audiobook_updated(state, audiobook_id).await.ok();
    Ok(())
}

#[derive(Debug, Deserialize)]
struct DbChapterParagraphs {
    #[serde(default)]
    paragraphs: Option<Vec<serde_json::Value>>,
}

async fn assert_voice_enabled(state: &AppState, voice_id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        id: Thing,
        enabled: bool,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT id, enabled FROM voice:`{voice_id}`"
        ))
        .await
        .map_err(|e| Error::Database(format!("voice lookup: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("voice lookup (decode): {e}")))?;
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| Error::Validation(format!("unknown voice `{voice_id}`")))?;
    if !row.enabled {
        return Err(Error::Validation(format!("voice `{voice_id}` is disabled")));
    }
    Ok(row.id.id.to_raw())
}

/// Validate that `id` refers to a podcast row owned by `user`. Rejects
/// unsafe characters before they're embedded in a `podcast:`<id>`` clause.
async fn assert_podcast_owned(state: &AppState, id: &str, user: &UserId) -> Result<()> {
    let valid = !id.is_empty()
        && id.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '-'
        });
    if !valid {
        return Err(Error::Validation(format!("invalid podcast id `{id}`")));
    }
    #[derive(Deserialize)]
    struct Row {
        owner: Thing,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!("SELECT owner FROM podcast:`{id}`"))
        .await
        .map_err(|e| Error::Database(format!("podcast owner: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("podcast owner (decode): {e}")))?;
    let row = rows.into_iter().next().ok_or_else(|| {
        Error::Validation(format!("unknown podcast `{id}`"))
    })?;
    if row.owner.id.to_raw() != user.0 {
        return Err(Error::Validation(format!("unknown podcast `{id}`")));
    }
    Ok(())
}

/// Validate that `name` references an entry in the curated
/// `audiobook_category` table. Empty / NULL slots are handled by the
/// caller — this only fires when the user supplied a non-empty value.
async fn assert_category_exists(state: &AppState, name: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Row {
        #[allow(dead_code)]
        name: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query("SELECT name FROM audiobook_category WHERE name = $n LIMIT 1")
        .bind(("n", name.to_string()))
        .await
        .map_err(|e| Error::Database(format!("category check: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("category check (decode): {e}")))?;
    if rows.is_empty() {
        return Err(Error::Validation(format!(
            "unknown category `{name}` — admin must add it first"
        )));
    }
    Ok(())
}

async fn assert_owner(state: &AppState, id: &str, user: &UserId) -> Result<()> {
    let book = load_audiobook(state, id).await?;
    if book.owner_id() != *user {
        return Err(Error::NotFound {
            resource: format!("audiobook:{id}"),
        });
    }
    Ok(())
}

async fn load_chapter_by_number(state: &AppState, audiobook_id: &str, n: i64) -> Result<DbChapter> {
    // Default to the audiobook's primary language. Editing endpoints
    // (patch/regenerate) currently target the primary version; translations
    // get their own update path.
    let book = load_audiobook(state, audiobook_id).await?;
    let lang = book.language.clone().unwrap_or_else(|| "en".to_string());
    let rows: Vec<DbChapter> = state
        .db()
        .inner()
        .query(format!(
            "SELECT * FROM chapter WHERE audiobook = audiobook:`{audiobook_id}` \
             AND number = $n AND language = $lang LIMIT 1"
        ))
        .bind(("n", n))
        .bind(("lang", lang))
        .await
        .map_err(|e| Error::Database(format!("load chapter: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("load chapter (decode): {e}")))?;
    rows.into_iter().next().ok_or(Error::NotFound {
        resource: format!("audiobook:{audiobook_id} chapter {n}"),
    })
}
