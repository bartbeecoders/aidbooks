use listenai_core::error::ErrorBody;
use utoipa::OpenApi;

use crate::handlers::health::{DbReadiness, Health, ReadinessReport};

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
    ),
    components(schemas(Health, ReadinessReport, DbReadiness, ErrorBody)),
    tags(
        (name = "system", description = "Health and readiness probes."),
    ),
)]
pub struct ApiDoc;
