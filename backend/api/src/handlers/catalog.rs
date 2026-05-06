//! Read-only catalogues for the create-flow UI: available voices and LLMs.
//! Admin-write endpoints will land in Phase 7.

use axum::{
    extract::{Path, State},
    Json,
};
use base64::Engine as _;
use hound::{SampleFormat, WavSpec, WavWriter};
use listenai_core::domain::{Llm, LlmProvider, LlmRole, Voice, VoiceGender};
use listenai_core::id::{LlmId, VoiceId};
use listenai_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::PathBuf;
use surrealdb::sql::Thing;
use utoipa::ToSchema;

use crate::auth::Authenticated;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct VoiceList {
    pub items: Vec<Voice>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LlmList {
    pub items: Vec<Llm>,
}

#[derive(Debug, Deserialize)]
struct DbVoice {
    id: Thing,
    name: String,
    provider: String,
    provider_voice_id: String,
    gender: String,
    accent: String,
    language: String,
    sample_url: Option<String>,
    enabled: bool,
    premium_only: bool,
}

#[derive(Debug, Deserialize)]
struct DbLlm {
    id: Thing,
    name: String,
    provider: String,
    model_id: String,
    context_window: i64,
    cost_prompt_per_1k: f64,
    cost_completion_per_1k: f64,
    #[serde(default)]
    cost_per_megapixel: f64,
    enabled: bool,
    default_for: Vec<String>,
    #[serde(default)]
    function: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default = "default_priority")]
    priority: i64,
}

fn default_priority() -> i64 {
    100
}

#[utoipa::path(
    get,
    path = "/voices",
    tag = "catalog",
    responses(
        (status = 200, description = "Enabled voices", body = VoiceList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list_voices(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
) -> ApiResult<Json<VoiceList>> {
    let rows: Vec<DbVoice> = state
        .db()
        .inner()
        .query("SELECT * FROM voice WHERE enabled = true ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("list voices: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list voices (decode): {e}")))?;

    let items = rows
        .into_iter()
        .map(|r| {
            Ok(Voice {
                id: VoiceId(r.id.id.to_raw()),
                name: r.name,
                provider: r.provider,
                provider_voice_id: r.provider_voice_id,
                gender: parse_gender(&r.gender)?,
                accent: r.accent,
                language: r.language,
                sample_url: r.sample_url,
                enabled: r.enabled,
                premium_only: r.premium_only,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(VoiceList { items }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VoicePreviewResponse {
    /// WAV file (mono, 16-bit PCM) encoded as standard base64. Playable
    /// directly via `<audio src="data:audio/wav;base64,…">`.
    pub audio_wav_base64: String,
    pub sample_rate_hz: u32,
    pub duration_ms: u64,
    pub mocked: bool,
}

/// Pick a short, narration-style preview line in the voice's own
/// language. Falls back to English for `multilingual` voices and any
/// language we haven't translated yet — the models accept English text
/// even on language-tagged voices, so the preview still produces sound
/// rather than failing.
fn preview_text_for_language(lang: &str) -> &'static str {
    // Match on the BCP-47 prefix (`nl-NL` → `nl`, `en-GB` → `en`) so we
    // don't have to enumerate every regional variant.
    let prefix = lang.split('-').next().unwrap_or("");
    match prefix {
        "nl" => "Hallo, dit is een korte voorproef van mijn stem. \
            Als je het mooi vindt klinken, kies mij om je luisterboek te vertellen.",
        "fr" => "Bonjour, voici un court aperçu de ma voix. \
            Si vous aimez son timbre, choisissez-moi pour narrer votre livre audio.",
        "de" => "Hallo, das ist eine kurze Hörprobe meiner Stimme. \
            Wenn dir der Klang gefällt, wähle mich als Erzähler für dein Hörbuch.",
        "es" => "Hola, esta es una breve muestra de mi voz. \
            Si te gusta cómo suena, elígeme para narrar tu audiolibro.",
        "it" => "Ciao, questa è una breve anteprima della mia voce. \
            Se ti piace come suono, scegli me per narrare il tuo audiolibro.",
        "pt" => "Olá, esta é uma breve amostra da minha voz. \
            Se você gostar de como soa, escolha-me para narrar seu audiolivro.",
        "ru" => "Здравствуйте, это короткий пример моего голоса. \
            Если вам нравится, как я звучу, выберите меня, чтобы озвучить вашу аудиокнигу.",
        "pl" => "Cześć, to krótka próbka mojego głosu. \
            Jeśli podoba ci się, jak brzmię, wybierz mnie do narracji twojego audiobooka.",
        "tr" => "Merhaba, bu sesimin kısa bir önizlemesi. \
            Sesimi beğendiyseniz, sesli kitabınızı anlatmam için beni seçin.",
        "sv" => "Hej, det här är ett kort smakprov på min röst. \
            Om du gillar hur jag låter, välj mig för att berätta din ljudbok.",
        "da" => "Hej, dette er en kort prøve på min stemme. \
            Hvis du kan lide, hvordan jeg lyder, så vælg mig til at fortælle din lydbog.",
        "fi" => "Hei, tämä on lyhyt näyte ääneeni. \
            Jos pidät siitä, miltä kuulostan, valitse minut kertomaan äänikirjasi.",
        "hu" => "Helló, ez egy rövid hangminta a hangomból. \
            Ha tetszik, ahogy szólok, válassz engem a hangoskönyved felolvasójának.",
        "ca" => "Hola, aquesta és una breu mostra de la meva veu. \
            Si t'agrada com sono, tria'm per narrar el teu audiollibre.",
        "id" => "Halo, ini adalah cuplikan singkat dari suara saya. \
            Jika kamu suka cara saya berbicara, pilih saya untuk menarasikan buku audiomu.",
        "vi" => "Xin chào, đây là một đoạn ngắn giọng nói của tôi. \
            Nếu bạn thích giọng tôi, hãy chọn tôi để kể sách nói của bạn.",
        "th" => "สวัสดี นี่คือตัวอย่างเสียงของฉันสั้น ๆ \
            ถ้าคุณชอบเสียงของฉัน เลือกฉันให้เล่าหนังสือเสียงของคุณ",
        "ar" => "مرحبًا، هذه عيّنة قصيرة من صوتي. \
            إذا أعجبك صوتي، اخترني لأروي كتابك الصوتي.",
        "hi" => "नमस्ते, यह मेरी आवाज़ का एक छोटा सा नमूना है। \
            अगर आपको मेरी आवाज़ पसंद आए, तो अपनी ऑडियोबुक सुनाने के लिए मुझे चुनें।",
        "bn" => "নমস্কার, এটি আমার কণ্ঠের একটি ছোট নমুনা। \
            যদি আপনি আমার কণ্ঠ পছন্দ করেন, আপনার অডিওবই বর্ণনার জন্য আমাকে বেছে নিন।",
        "zh" => "你好，这是我声音的简短样本。如果你喜欢我的声音，请选我为你朗读有声书。",
        "ja" => "こんにちは、これは私の声の短いサンプルです。気に入ったら、あなたのオーディオブックの語り手に私を選んでください。",
        "ko" => "안녕하세요, 제 목소리의 짧은 샘플입니다. 마음에 드시면 오디오북의 내레이터로 저를 선택해 주세요.",
        // English (en, en-GB, en-US, en-IE, en-ZA) and the catch-all,
        // including `multilingual` voices which the picker uses for any
        // audiobook language.
        _ => "Hello, this is a short preview of my voice. \
            If you like how I sound, pick me to narrate your audiobook.",
    }
}

#[utoipa::path(
    get,
    path = "/voices/{id}/preview",
    tag = "catalog",
    params(("id" = String, Path, description = "Voice id from /voices")),
    responses(
        (status = 200, description = "Synthesised preview clip", body = VoicePreviewResponse),
        (status = 401, description = "Unauthenticated"),
        (status = 404, description = "Unknown voice id"),
        (status = 502, description = "TTS upstream error"),
    ),
    security(("bearer" = []))
)]
pub async fn preview_voice(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
    Path(id): Path<String>,
) -> ApiResult<Json<VoicePreviewResponse>> {
    // Reject anything that wouldn't be a SurrealDB record id; this also
    // makes the value safe to interpolate into the cache file path below.
    if !is_safe_voice_id(&id) {
        return Err(Error::Validation("invalid voice id".into()).into());
    }

    // Cache previews on disk so a voice is only synthesised once, even
    // across server restarts. The TTS upstream is rate-limited and the
    // preview text is fixed, so this is safe to memoise indefinitely.
    let cache_path = preview_cache_path(&state, &id);
    if cache_path.exists() {
        if let Some(resp) = read_cached_preview(&cache_path) {
            return Ok(Json(resp));
        }
    }

    let row: Option<DbVoice> = state
        .db()
        .inner()
        .query(format!("SELECT * FROM voice:`{id}` WHERE enabled = true"))
        .await
        .map_err(|e| Error::Database(format!("preview_voice: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("preview_voice (decode): {e}")))?;
    let voice = row.ok_or_else(|| Error::NotFound {
        resource: format!("voice:{id}"),
    })?;

    // Voices flagged as language-specific (e.g. `nl-NL`) sound off when
    // forced through the configured TTS language, so prefer the voice's
    // own tag and fall back to the global config for `multilingual`.
    let language = if voice.language == "multilingual" || voice.language.is_empty() {
        state.config().xai_tts_language.clone()
    } else {
        voice.language.clone()
    };

    let preview_text = preview_text_for_language(&language);
    let pcm = state
        .tts()
        .synthesize(preview_text, &voice.provider_voice_id, &language)
        .await?;
    let wav = encode_wav(&pcm.samples, pcm.sample_rate_hz)?;

    // Write the WAV to disk (best effort). Mock TTS output is excluded so
    // a dev session that flips between mock/live providers doesn't pin a
    // bogus sample on the next real run.
    if !pcm.mocked {
        if let Err(e) = write_cached_preview(&cache_path, &wav) {
            tracing::warn!("voice preview cache write failed for {id}: {e}");
        }
    }

    let audio_wav_base64 = base64::engine::general_purpose::STANDARD.encode(&wav);
    Ok(Json(VoicePreviewResponse {
        audio_wav_base64,
        sample_rate_hz: pcm.sample_rate_hz,
        duration_ms: pcm.duration_ms(),
        mocked: pcm.mocked,
    }))
}

fn is_safe_voice_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 120
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn preview_cache_path(state: &AppState, voice_id: &str) -> PathBuf {
    state
        .config()
        .storage_path
        .join("voice-previews")
        .join(format!("{voice_id}.wav"))
}

fn read_cached_preview(path: &std::path::Path) -> Option<VoicePreviewResponse> {
    let bytes = std::fs::read(path).ok()?;
    // Parse WAV header so the response carries the correct sample rate
    // and duration without re-decoding the audio body.
    let reader = hound::WavReader::new(Cursor::new(&bytes)).ok()?;
    let spec = reader.spec();
    let frames = reader.duration() as u64;
    let duration_ms = if spec.sample_rate == 0 {
        0
    } else {
        frames * 1000 / spec.sample_rate as u64
    };
    let audio_wav_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(VoicePreviewResponse {
        audio_wav_base64,
        sample_rate_hz: spec.sample_rate,
        duration_ms,
        mocked: false,
    })
}

fn write_cached_preview(path: &std::path::Path, wav: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Atomic-ish: write to a temp file then rename so an interrupted
    // write doesn't leave a half-baked WAV that breaks the cached path.
    let tmp = path.with_extension("wav.tmp");
    std::fs::write(&tmp, wav)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn encode_wav(samples: &[i16], sample_rate_hz: u32) -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf: Vec<u8> = Vec::with_capacity(samples.len() * 2 + 44);
    {
        let cursor = Cursor::new(&mut buf);
        let mut w = WavWriter::new(cursor, spec)
            .map_err(|e| Error::Other(anyhow::anyhow!("wav header: {e}")))?;
        for s in samples {
            w.write_sample(*s)
                .map_err(|e| Error::Other(anyhow::anyhow!("wav write: {e}")))?;
        }
        w.finalize()
            .map_err(|e| Error::Other(anyhow::anyhow!("wav finalize: {e}")))?;
    }
    Ok(buf)
}

#[utoipa::path(
    get,
    path = "/llms",
    tag = "catalog",
    responses(
        (status = 200, description = "Enabled LLM configs", body = LlmList),
        (status = 401, description = "Unauthenticated")
    ),
    security(("bearer" = []))
)]
pub async fn list_llms(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
) -> ApiResult<Json<LlmList>> {
    let rows: Vec<DbLlm> = state
        .db()
        .inner()
        .query("SELECT * FROM llm WHERE enabled = true ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("list llms: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list llms (decode): {e}")))?;

    let items = rows
        .into_iter()
        .map(|r| {
            Ok(Llm {
                id: LlmId(r.id.id.to_raw()),
                name: r.name,
                provider: parse_provider(&r.provider)?,
                model_id: r.model_id,
                context_window: r.context_window as u32,
                cost_prompt_per_1k: r.cost_prompt_per_1k,
                cost_completion_per_1k: r.cost_completion_per_1k,
                cost_per_megapixel: r.cost_per_megapixel,
                enabled: r.enabled,
                default_for: r.default_for.iter().filter_map(|s| parse_role(s)).collect(),
                function: r.function.filter(|s| !s.trim().is_empty()),
                languages: r.languages,
                priority: r.priority as i32,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(LlmList { items }))
}

/// Public listing of audiobook categories — used by the New Audiobook
/// form and the per-book category picker. Admin endpoints (`/admin/...`)
/// also expose this list with usage counts.
#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookCategoryName {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AudiobookCategoryNameList {
    pub items: Vec<AudiobookCategoryName>,
}

#[utoipa::path(
    get, path = "/audiobook-categories", tag = "audiobook",
    responses(
        (status = 200, body = AudiobookCategoryNameList),
        (status = 401)
    ),
    security(("bearer" = []))
)]
pub async fn list_audiobook_categories(
    State(state): State<AppState>,
    Authenticated(_user): Authenticated,
) -> ApiResult<Json<AudiobookCategoryNameList>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query("SELECT name FROM audiobook_category ORDER BY name ASC")
        .await
        .map_err(|e| Error::Database(format!("list categories: {e}")))?
        .take(0)
        .map_err(|e| Error::Database(format!("list categories (decode): {e}")))?;
    Ok(Json(AudiobookCategoryNameList {
        items: rows
            .into_iter()
            .map(|r| AudiobookCategoryName { name: r.name })
            .collect(),
    }))
}

fn parse_gender(s: &str) -> Result<VoiceGender> {
    Ok(match s {
        "female" => VoiceGender::Female,
        "male" => VoiceGender::Male,
        "neutral" => VoiceGender::Neutral,
        other => return Err(Error::Database(format!("unknown gender `{other}`"))),
    })
}

fn parse_provider(s: &str) -> Result<LlmProvider> {
    Ok(match s {
        "open_router" => LlmProvider::OpenRouter,
        "xai" => LlmProvider::Xai,
        other => return Err(Error::Database(format!("unknown provider `{other}`"))),
    })
}

fn parse_role(s: &str) -> Option<LlmRole> {
    Some(match s {
        "outline" => LlmRole::Outline,
        "chapter" => LlmRole::Chapter,
        "title" => LlmRole::Title,
        "random_topic" => LlmRole::RandomTopic,
        "moderation" => LlmRole::Moderation,
        "cover_art" => LlmRole::CoverArt,
        "translate" => LlmRole::Translate,
        "manim_code" => LlmRole::ManimCode,
        "voice_extract" => LlmRole::VoiceExtract,
        _ => return None,
    })
}
