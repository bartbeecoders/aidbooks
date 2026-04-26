use listenai_core::domain::{
    AudiobookLength, AudiobookStatus, ChapterStatus, JobKind, JobStatus, Llm, LlmProvider, LlmRole,
    User, UserRole, UserTier, Voice, VoiceGender,
};
use listenai_core::error::ErrorBody;
use listenai_jobs::hub::{JobSnapshot, ProgressEvent};
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use crate::handlers::admin::{
    AdminJobList, AdminJobRow, AdminLlmList, AdminLlmRow, AdminUserList, AdminUserRow,
    AdminVoiceList, AdminVoiceRow, CreateLlmRequest, RevokeSessionsResponse, SystemOverview,
    TestLlmRequest, TestLlmResponse, TestVoiceRequest, TestVoiceResponse, UpdateLlmRequest,
    UpdateUserRequest, UpdateVoiceRequest,
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
use crate::handlers::jobs::AudiobookJobList;
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
        crate::handlers::audiobook::regenerate_cover,
        crate::handlers::audiobook::translate,
        crate::handlers::stream::chapter_audio,
        crate::handlers::stream::chapter_waveform,
        crate::handlers::stream::cover,
        crate::handlers::cover::preview,
        crate::handlers::jobs::list_for_audiobook,
        crate::handlers::topics::random,
        crate::handlers::catalog::list_voices,
        crate::handlers::catalog::list_llms,
        // --- Phase 7: admin ---
        crate::handlers::admin::system_overview,
        crate::handlers::admin::list_llms,
        crate::handlers::admin::patch_llm,
        crate::handlers::admin::create_llm,
        crate::handlers::admin::list_voices,
        crate::handlers::admin::patch_voice,
        crate::handlers::admin::list_users,
        crate::handlers::admin::patch_user,
        crate::handlers::admin::revoke_sessions,
        crate::handlers::admin::list_jobs,
        crate::handlers::admin::retry_job,
        crate::handlers::admin::test_llm,
        crate::handlers::admin::test_voice,
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
        crate::handlers::audiobook::TranslateRequest,
        crate::handlers::audiobook::TranslateResponse,
        // cover art
        crate::handlers::cover::CoverPreviewRequest,
        crate::handlers::cover::CoverPreviewResponse,
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
        // jobs + progress
        JobKind,
        JobStatus,
        JobSnapshot,
        AudiobookJobList,
        ProgressEvent,
        // admin
        SystemOverview,
        AdminLlmRow,
        AdminLlmList,
        UpdateLlmRequest,
        CreateLlmRequest,
        AdminVoiceRow,
        AdminVoiceList,
        UpdateVoiceRequest,
        AdminUserRow,
        AdminUserList,
        UpdateUserRequest,
        RevokeSessionsResponse,
        AdminJobRow,
        AdminJobList,
        TestLlmRequest,
        TestLlmResponse,
        TestVoiceRequest,
        TestVoiceResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "system", description = "Health and readiness probes."),
        (name = "auth", description = "Authentication, tokens, and current user."),
        (name = "audiobook", description = "Audiobook CRUD and content generation."),
        (name = "cover-art", description = "Image generation for audiobook covers."),
        (name = "topics", description = "Topic helpers (random, etc)."),
        (name = "catalog", description = "Voices and LLMs available to the UI."),
        (name = "jobs", description = "Durable job inspection (WebSocket at /ws/audiobook/:id)."),
        (name = "admin", description = "Admin-only: runtime-editable LLMs, voices, users, jobs."),
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
