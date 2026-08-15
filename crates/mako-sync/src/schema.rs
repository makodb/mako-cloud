use std::{error::Error, fmt};

use mako_api::{ApiError, ApiErrorEnvelope, ErrorCode, RetryAdvice, SafeDetail};
use mako_documents::SchemaVersion;

use crate::AuthenticatedReplicationContext;

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaMigrationRequired {
    required_schema_version: SchemaVersion,
    request_id: String,
}

impl SchemaMigrationRequired {
    #[must_use]
    pub const fn required_schema_version(&self) -> SchemaVersion {
        self.required_schema_version
    }

    #[must_use]
    pub fn api_error(&self) -> ApiErrorEnvelope {
        ApiErrorEnvelope::new(
            ApiError::new(
                ErrorCode::SchemaMismatch,
                "collection schema migration is required",
                &self.request_id,
                RetryAdvice::Never,
            )
            .with_detail(
                "requiredSchemaVersion",
                SafeDetail::String(self.required_schema_version.get().to_string()),
            ),
        )
    }
}

impl fmt::Display for SchemaMigrationRequired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "client schema migration required; required schema version is {}",
            self.required_schema_version.get()
        )
    }
}

impl Error for SchemaMigrationRequired {}

pub(crate) fn require_compatible_schema(
    required: SchemaVersion,
    requested: u64,
    context: &AuthenticatedReplicationContext,
) -> Result<(), SchemaMigrationRequired> {
    if required.get() == requested {
        return Ok(());
    }
    Err(SchemaMigrationRequired {
        required_schema_version: required,
        request_id: context.request_id().to_owned(),
    })
}
