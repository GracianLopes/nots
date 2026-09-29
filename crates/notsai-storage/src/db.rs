//! SQLite pool construction and migration application.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use notsai_core::CoreError;

/// Convert an [`sqlx::Error`] into the shared domain error type.
///
/// `sqlx::Error` is a foreign type, so the orphan rule prevents implementing
/// `From<sqlx::Error>` for `CoreError`; every sqlx call is mapped through this
/// helper instead.
pub(crate) fn sqlite_error(source: sqlx::Error) -> CoreError {
    CoreError::storage(format!("sqlite: {source}"))
}

/// Open a connection pool for the database at `path`, applying migrations.
pub async fn open(path: &Path) -> Result<SqlitePool, CoreError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(sqlite_error)?;

    migrate(&pool).await?;
    Ok(pool)
}

/// Open an ephemeral in-memory pool, applying migrations.
///
/// A single pooled connection keeps the in-memory schema alive for the life of
/// the pool, so callers must retain the returned pool for as long as the data
/// is needed.
pub async fn open_in_memory() -> Result<SqlitePool, CoreError> {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .journal_mode(SqliteJournalMode::Memory)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(sqlite_error)?;

    migrate(&pool).await?;
    Ok(pool)
}

/// Apply all pending embedded migrations.
async fn migrate(pool: &SqlitePool) -> Result<(), CoreError> {
    sqlx::migrate!()
        .run(pool)
        .await
        .map_err(|e| CoreError::storage(format!("migration failed: {e}")))
}
