use listenai_core::domain::{
    AudiobookLength, AudiobookStatus, ChapterStatus, Llm, LlmProvider, LlmRole, User, UserRole,
    UserTier, Voice, VoiceGender,
};
use listenai_core::error::ErrorBody;
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use crate::handlers::audiobook::{
    AudiobookDetail, AudiobookList, AudiobookSummary, ChapterSummary, CreateAudiobookRequest,
    UpdateAudiobookRequest, UpdateChapterRequest,
};
use crate::handlers::auth::{
    AuthResponse, LoginRequest, LogoutRequest, RefreshRequest, RegisterRequest,
};
use crate::handlers::catalog::{LlmList, VoiceList};
use crate::handlers::health::{DbReadiness, Health, ReadinessReport};
use crate::handlers::me::{MeResponse, UpdateMeRequest};
use crate::handlers::topics::{RandomTopicRequest, RandomTopicResponse};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "ListenAI API",
        description = "REST API for the ListenAI audiobook generator.",
        version = env!("CARGO_PKG_VERSION"),
        license(name = "MIT")
    ),
    paths(
        crate::handlers::health::health,
        crate::handlers::health::ready,
        crate::handlers::auth::register,
        crate::handlers::auth::login,
        crate::handlers::auth::refresh,
        crate::handlers::auth::logout,
        crate::handlers::me::get_me,
        crate::handlers::me::patch_me,
        crate::handlers::audiobook::create,
        crate::handlers::audiobook::list,
        crate::handlers::audiobook::get_one,
        crate::handlers::audiobook::patch,
        crate::handlers::audiobook::delete,
        crate::handlers::audiobook::generate_chapters,
        crate::handlers::audiobook::patch_chapter,
        crate::handlers::audiobook::regenerate_chapter,
        crate::handlers::audiobook::generate_audio,
        crate::handlers::audiobook::regenerate_chapter_audio,
        crate::handlers::stream::chapter_audio,
        crate::handlers::stream::chapter_waveform,
        crate::handlers::topics::random,
        crate::handlers::catalog::list_voices,
        crate::handlers::catalog::list_llms,
    ),
    components(schemas(
        Health,
        ReadinessReport,
        DbReadiness,
        ErrorBody,
        // auth + user
        User,
        UserRole,
        UserTier,
        RegisterRequest,
        LoginRequest,
        RefreshRequest,
        LogoutRequest,
        AuthResponse,
        MeResponse,
        UpdateMeRequest,
        // audiobooks
        AudiobookLength,
        AudiobookStatus,
        ChapterStatus,
        AudiobookSummary,
        AudiobookDetail,
        AudiobookList,
        ChapterSummary,
        CreateAudiobookRequest,
        UpdateAudiobookRequest,
        UpdateChapterRequest,
        // topics
        RandomTopicRequest,
        RandomTopicResponse,
        // catalog
        Voice,
        VoiceGender,
        Llm,
        LlmProvider,
        LlmRole,
        VoiceList,
        LlmList,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "system", description = "Health and readiness probes."),
        (name = "auth", description = "Authentication, tokens, and current user."),
        (name = "audiobook", description = "Audiobook CRUD and content generation."),
        (name = "topics", description = "Topic helpers (random, etc)."),
        (name = "catalog", description = "Voices and LLMs available to the UI."),
    ),
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }
    }
}
