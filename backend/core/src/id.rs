//! Strongly-typed identifier newtypes. Every row in SurrealDB has a
//! `table:ulid`-style id; this module wraps the string portion for type
//! safety across layers without leaking SurrealDB's `Thing` type.

use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $table:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub const TABLE: &'static str = $table;

            pub fn new() -> Self {
                Self(Uuid::new_v4().simple().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}:{}", Self::TABLE, self.0)
            }
        }
    };
}

define_id!(
    /// Identifier for a user record.
    UserId,
    "user"
);
define_id!(
    /// Identifier for an audiobook record.
    AudiobookId,
    "audiobook"
);
define_id!(
    /// Identifier for a chapter record.
    ChapterId,
    "chapter"
);
define_id!(
    /// Identifier for a voice record.
    VoiceId,
    "voice"
);
define_id!(
    /// Identifier for an LLM configuration record.
    LlmId,
    "llm"
);
define_id!(
    /// Identifier for a job record.
    JobId,
    "job"
);
