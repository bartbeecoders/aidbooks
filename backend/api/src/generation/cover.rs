//! Cover-art generation via OpenRouter.
//!
//! Uses an image-capable LLM (e.g. `google/gemini-2.5-flash-image`) selected
//! by the `cover_art` role. The model returns a base64-encoded PNG; callers
//! receive the decoded bytes.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use listenai_core::domain::{LlmRole, PromptRole};
use listenai_core::id::UserId;
use listenai_core::{Error, Result};
use serde::Deserialize;

use crate::generation::outline::log_generation_event;
use crate::llm::{pick_llm_for_role, ChatMessage, ChatRequest, ChatResponse};
use crate::state::AppState;

const SYSTEM: &str = "You generate audiobook cover artwork. Produce a single \
square cover image. No text, no captions, no chapter numbers — purely \
illustrative. Match the requested visual style precisely.";

/// Default style applied when the caller doesn't specify one. Preserves the
/// pre-existing behaviour (broadly cinematic compositions) for older books
/// that haven't picked a style yet.
pub const DEFAULT_ART_STYLE: &str = "cinematic";

/// Generate a cover image for the given topic + optional genre + style.
/// Returns the raw image bytes (typically PNG).
///
/// `llm_id_override` short-circuits the picker — when supplied (and the
/// referenced LLM exists and is enabled) its `model_id` is used directly,
/// otherwise the standard `pick_model_for_role` path runs.
///
/// `user` and `audiobook_id` are persisted with the cost log; `audiobook_id`
/// is `None` for the stateless `/cover-art/preview` path.
#[allow(clippy::too_many_arguments)]
pub async fn generate(
    state: &AppState,
    user: &UserId,
    audiobook_id: Option<&str>,
    topic: &str,
    genre: Option<&str>,
    art_style: Option<&str>,
    llm_id_override: Option<&str>,
) -> Result<Vec<u8>> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(Error::Validation("topic must not be empty".into()));
    }

    let (llm_id, provider, model) = resolve_model(state, llm_id_override).await?;
    let prompt = build_prompt(topic, genre, art_style);
    request_image(
        state,
        user,
        audiobook_id,
        &llm_id,
        PromptRole::Cover,
        &provider,
        model,
        &prompt,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn generate_chapter_art(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    art_style: Option<&str>,
    llm_id_override: Option<&str>,
    chapter_number: u32,
    chapter_title: &str,
    synopsis: Option<&str>,
    body_md: Option<&str>,
) -> Result<Vec<u8>> {
    let (llm_id, provider, model) = resolve_model(state, llm_id_override).await?;
    let prompt = build_chapter_prompt(
        book_title,
        book_topic,
        genre,
        art_style,
        chapter_number,
        chapter_title,
        synopsis,
        body_md,
    );
    request_image(
        state,
        user,
        Some(audiobook_id),
        &llm_id,
        PromptRole::Cover,
        &provider,
        model,
        &prompt,
    )
    .await
}

/// Generate one paragraph-level illustration. `scene_description` should
/// already be a vivid scene from the extract pass — the prompt builder
/// frames it with style + genre context. `ordinal` (1-based) lets us ask
/// the model for slightly different framings of the same scene when the
/// caller requested multiple tiles per paragraph.
#[allow(clippy::too_many_arguments)]
pub async fn generate_paragraph_image(
    state: &AppState,
    user: &UserId,
    audiobook_id: &str,
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    art_style: Option<&str>,
    llm_id_override: Option<&str>,
    chapter_title: &str,
    paragraph_text: &str,
    scene_description: &str,
    ordinal: u32,
    total_ordinals: u32,
) -> Result<Vec<u8>> {
    if ordinal == 0 || total_ordinals == 0 || ordinal > total_ordinals {
        return Err(Error::Validation(format!(
            "invalid paragraph ordinal {ordinal}/{total_ordinals}"
        )));
    }
    let (llm_id, provider, model) = resolve_model(state, llm_id_override).await?;
    let prompt = build_paragraph_prompt(
        book_title,
        book_topic,
        genre,
        art_style,
        chapter_title,
        paragraph_text,
        scene_description,
        ordinal,
        total_ordinals,
    );
    request_image(
        state,
        user,
        Some(audiobook_id),
        &llm_id,
        PromptRole::ParagraphImage,
        &provider,
        model,
        &prompt,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn request_image(
    state: &AppState,
    user: &UserId,
    audiobook_id: Option<&str>,
    llm_id: &str,
    role: PromptRole,
    provider: &str,
    model: String,
    prompt: &str,
) -> Result<Vec<u8>> {
    // Single high-level log per image so admins can trace which row the
    // picker chose. The chat-dispatch log fires later for OpenRouter, but
    // the xAI image branch bypasses that path — log here so both providers
    // produce a consistent breadcrumb.
    let role_str = match role {
        PromptRole::Cover => "cover",
        PromptRole::ParagraphImage => "paragraph_image",
        _ => "image",
    };
    tracing::info!(
        llm_id = %llm_id,
        provider = %provider,
        model = %model,
        role = role_str,
        audiobook = audiobook_id.unwrap_or(""),
        "image gen: dispatching"
    );

    // xAI image gen uses a different endpoint (`/images/generations`)
    // than chat completions and bills per generated image — branch off
    // before the chat-shaped path.
    if provider == "xai" {
        return xai_image(state, user, audiobook_id, llm_id, role, &model, prompt).await;
    }

    let messages = vec![
        ChatMessage::system(SYSTEM),
        ChatMessage::user(prompt),
    ];
    let provider_owned = provider.to_string();
    let mk_req = |modalities: Vec<String>| ChatRequest {
        model: model.clone(),
        messages: messages.clone(),
        temperature: Some(1.0),
        // Image-capable models embed the image in `message.images[]` (Gemini)
        // or as a `data:image/png;base64,…` block, both of which count
        // toward the response budget. 1024 was tight enough that some
        // base64 payloads got truncated to "neither text nor image".
        max_tokens: Some(8192),
        json_mode: None,
        modalities: Some(modalities),
        provider: Some(provider_owned.clone()),
    };

    // Most OpenRouter image models are chat-shaped and emit text alongside
    // the image (Gemini, GPT-image), so we ask for both. Image-only models
    // (FLUX, Riverflow, Seedream, …) reject `["image","text"]` with a 404
    // — fall back to image-only and retry once.
    let resp = match state.llm().chat(&mk_req(vec!["image".into(), "text".into()])).await {
        Ok(r) => r,
        Err(Error::Upstream(msg)) if is_modality_mismatch(&msg) => {
            tracing::info!(
                model = %model,
                "cover: retrying with modalities=[image] only — model is image-output only",
            );
            state.llm().chat(&mk_req(vec!["image".into()])).await?
        }
        Err(e) => {
            // Log the failed attempt so admins still see the cost (often $0
            // for a 4xx, but useful as a record of attempts).
            log_generation_event(
                state,
                user,
                audiobook_id,
                llm_id,
                role,
                &empty_response(),
                Some(&e.to_string()),
            )
            .await
            .ok();
            return Err(e);
        }
    };

    log_generation_event(state, user, audiobook_id, llm_id, role, &resp, None)
        .await
        .ok();

    let b64 = resp.image_base64.ok_or_else(|| {
        Error::Upstream(
            "openrouter: model did not return an image — check that the \
             model selected for `cover_art` supports image output"
                .into(),
        )
    })?;

    let bytes = B64
        .decode(b64.as_bytes())
        .map_err(|e| Error::Upstream(format!("decode image base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Upstream("openrouter: empty image payload".into()));
    }
    Ok(bytes)
}

/// xAI image-gen path. xAI charges per image; we read the LLM row's
/// `cost_per_megapixel` (which the admin form pre-fills as $/image from
/// the picker) and stamp it on the logged ChatResponse so the cost badge
/// counts xAI image spend the same way OpenRouter image cost is counted.
#[allow(clippy::too_many_arguments)]
async fn xai_image(
    state: &AppState,
    user: &UserId,
    audiobook_id: Option<&str>,
    llm_id: &str,
    role: PromptRole,
    model: &str,
    prompt: &str,
) -> Result<Vec<u8>> {
    let b64 = match state.llm().generate_xai_image(model, prompt).await {
        Ok(b) => b,
        Err(e) => {
            log_generation_event(
                state,
                user,
                audiobook_id,
                llm_id,
                role,
                &empty_response(),
                Some(&e.to_string()),
            )
            .await
            .ok();
            return Err(e);
        }
    };

    let per_image = lookup_cost_per_image(state, llm_id).await.unwrap_or(0.0);
    let resp = ChatResponse {
        content: String::new(),
        image_base64: Some(b64.clone()),
        usage: crate::llm::ChatUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost: per_image,
        },
        mocked: false,
    };
    log_generation_event(state, user, audiobook_id, llm_id, role, &resp, None)
        .await
        .ok();

    let bytes = B64
        .decode(b64.as_bytes())
        .map_err(|e| Error::Upstream(format!("decode image base64: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Upstream("xai image gen: empty payload".into()));
    }
    Ok(bytes)
}

async fn lookup_cost_per_image(state: &AppState, llm_id: &str) -> Option<f64> {
    if !is_safe_id(llm_id) {
        return None;
    }
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        cost_per_megapixel: f64,
    }
    let rows: Vec<Row> = state
        .db()
        .inner()
        .query(format!(
            "SELECT cost_per_megapixel FROM llm:`{llm_id}`"
        ))
        .await
        .ok()?
        .take(0)
        .ok()?;
    rows.into_iter().next().map(|r| r.cost_per_megapixel)
}

fn build_prompt(topic: &str, genre: Option<&str>, art_style: Option<&str>) -> String {
    let style = resolve_style(art_style);
    let genre_line = match genre.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => format!("Genre: {g}\n"),
        None => String::new(),
    };
    format!(
        "Audiobook cover artwork.\n\
         Topic: {topic}\n\
         {genre_line}\
         Visual style: {style}\n\
         Compose a striking, atmospheric image that captures the mood, \
         executed entirely in the requested visual style. Square format. \
         No lettering of any kind."
    )
}

#[allow(clippy::too_many_arguments)]
fn build_chapter_prompt(
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    art_style: Option<&str>,
    chapter_number: u32,
    chapter_title: &str,
    synopsis: Option<&str>,
    body_md: Option<&str>,
) -> String {
    let excerpt = body_md
        .map(|b| b.chars().take(1200).collect::<String>())
        .filter(|b| !b.trim().is_empty());
    let style = resolve_style(art_style);
    format!(
        "Audiobook chapter artwork.\n\
         Book title: {book_title}\nBook topic: {book_topic}\nGenre: {genre}\n\
         Visual style: {style}\n\
         Chapter {chapter_number}: {chapter_title}\nSynopsis: {synopsis}\nExcerpt: {excerpt}\n\
         Compose a single illustration for this chapter, executed entirely \
         in the requested visual style. Square format. No lettering, no \
         captions, no numbers.",
        genre = genre.unwrap_or("any"),
        synopsis = synopsis.unwrap_or(""),
        excerpt = excerpt.as_deref().unwrap_or(""),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_paragraph_prompt(
    book_title: &str,
    book_topic: &str,
    genre: Option<&str>,
    art_style: Option<&str>,
    chapter_title: &str,
    paragraph_text: &str,
    scene_description: &str,
    ordinal: u32,
    total_ordinals: u32,
) -> String {
    let style = resolve_style(art_style);
    // Cap the raw paragraph excerpt so the prompt budget stays predictable.
    // The scene description is what the image model is really keying on;
    // the paragraph is included as flavour/context.
    let excerpt: String = paragraph_text.chars().take(800).collect();
    let framing = if total_ordinals == 1 {
        "Compose a single illustration".to_string()
    } else {
        format!(
            "Compose illustration {ordinal} of {total_ordinals} for this same \
             scene — vary the camera angle, framing, or moment so the set \
             feels like a small storyboard rather than duplicates"
        )
    };
    format!(
        "Audiobook paragraph illustration.\n\
         Book title: {book_title}\nBook topic: {book_topic}\nGenre: {genre}\n\
         Visual style: {style}\n\
         Chapter: {chapter_title}\n\
         Scene to depict: {scene_description}\n\
         Paragraph context: {excerpt}\n\
         {framing}, executed entirely in the requested visual style. Square \
         format. No lettering, no captions, no numbers.",
        genre = genre.unwrap_or("any"),
    )
}

fn resolve_style(art_style: Option<&str>) -> &str {
    match art_style.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => DEFAULT_ART_STYLE,
    }
}

/// Pick `(llm_id, model_id)` for the cover request.
///
/// If `llm_id_override` names an enabled LLM row, return that row's id and
/// `model_id` — this is how the cover-art picker on the New Audiobook /
/// Book Detail UI flows through. Otherwise fall back to the standard
/// `pick_llm_for_role` path so legacy callers keep working.
/// Returns `(llm_id, provider, model_id)` for the cover request — provider
/// is needed at the chat layer to dispatch to the right host.
async fn resolve_model(
    state: &AppState,
    llm_id_override: Option<&str>,
) -> Result<(String, String, String)> {
    let id = llm_id_override.map(str::trim).filter(|s| !s.is_empty());
    if let Some(id) = id {
        if !is_safe_id(id) {
            return Err(Error::Validation(format!("invalid llm_id `{id}`")));
        }
        #[derive(Debug, Deserialize)]
        struct Row {
            model_id: String,
            enabled: bool,
            #[serde(default)]
            provider: Option<String>,
        }
        let rows: Vec<Row> = state
            .db()
            .inner()
            .query(format!(
                "SELECT model_id, enabled, provider FROM llm:`{id}`"
            ))
            .await
            .map_err(|e| Error::Database(format!("resolve cover model: {e}")))?
            .take(0)
            .map_err(|e| Error::Database(format!("resolve cover model (decode): {e}")))?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| Error::Validation(format!("unknown llm `{id}`")))?;
        if !row.enabled {
            return Err(Error::Validation(format!("llm `{id}` is disabled")));
        }
        let provider = row.provider.unwrap_or_else(|| "open_router".into());
        return Ok((id.to_string(), provider, row.model_id));
    }
    let picked = pick_llm_for_role(state, LlmRole::CoverArt).await?;
    Ok((picked.llm_id, picked.provider, picked.model_id))
}

/// Empty `ChatResponse` for failure-path logging — keeps the log row's
/// shape consistent without inventing fake usage numbers.
fn empty_response() -> ChatResponse {
    ChatResponse {
        content: String::new(),
        image_base64: None,
        usage: Default::default(),
        mocked: false,
    }
}

/// `true` when an upstream error string indicates the request's `modalities`
/// don't match any of the model's available endpoints. OpenRouter returns
/// 404 with a message like
/// `No endpoints found that support the requested output modalities: image, text`
/// for image-only models when we ask for image+text.
fn is_modality_mismatch(msg: &str) -> bool {
    msg.contains("No endpoints found that support the requested output modalities")
}

/// Same charset rule as `is_valid_llm_id` in the admin module — keeps
/// embedded `llm:`<id>`` safe from injection.
fn is_safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
