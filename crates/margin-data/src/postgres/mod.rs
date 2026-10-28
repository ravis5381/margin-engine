//! PostgreSQL catalog. Connection and migrations live here. Dataset rows,
//! archived-file lookup, and the margin quote each have their own module.

mod catalog;
mod files;
mod margin;

pub(crate) use catalog::{DatasetRow, dataset_from_row, select_columns};
pub(crate) use files::one_auxiliary_hash;
pub use margin::StoredSpanQuote;
pub(crate) use margin::{ChosenSpan, margin_rejected, price_chosen_span};

use crate::DataError;
use crate::span::index::SpanIndexCache;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    span_indexes: Arc<SpanIndexCache>,
}

impl PgStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            span_indexes: SpanIndexCache::open(),
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

pub async fn connect(
    url: &str,
    max_connections: u32,
    acquire_timeout: Duration,
) -> Result<PgPool, DataError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(acquire_timeout)
        .connect(url)
        .await
        .map_err(|error| {
            tracing::error!(
                error = %crate::redact::redact(&error.to_string(), &[url]),
                "database connection failed"
            );
            DataError::Database {
                detail: "database connection failed".to_owned(),
            }
        })
}

pub async fn migrate(pool: &PgPool) -> Result<(), DataError> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "database migration failed");
            DataError::Database {
                detail: "database migration failed".to_owned(),
            }
        })
}

pub(crate) fn db_error_message(error: sqlx::Error) -> DataError {
    tracing::error!(error = %error, "database query failed");
    DataError::Database {
        detail: "database query failed".to_owned(),
    }
}
