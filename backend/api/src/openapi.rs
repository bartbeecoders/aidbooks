use listenai_core::domain::{User, UserRole, UserTier};
use listenai_core::error::ErrorBody;
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use crate::handlers::auth::{
    AuthResponse, LoginRequest, LogoutRequest, RefreshRequest, RegisterRequest,
};
use crate::handlers::health::{DbReadiness, Health, ReadinessReport};
use crate::handlers::me::{MeResponse, UpdateMeRequest};

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
    ),
    components(schemas(
        Health,
        ReadinessReport,
        DbReadiness,
        ErrorBody,
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
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "system", description = "Health and readiness probes."),
        (name = "auth", description = "Authentication, tokens, and current user."),
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
