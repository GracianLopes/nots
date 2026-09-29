//! notsai-storage — SQLite persistence, raw/derived data separation and
//! retention enforcement (FULL_RECORDING / SMART_NOTES / NOTES_ONLY).

#![forbid(unsafe_code)]

mod db;
mod models;
mod settings;
mod store;

pub use settings::{FileSettingsRepository, SETTINGS_FILENAME};
pub use store::{SqliteStore, MAX_SEARCH_HITS, RECORDING_FILENAME, TRANSCRIPT_FILENAME};
