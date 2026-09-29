//! File-backed settings persistence.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::Mutex;

use notsai_core::{AppSettings, CoreError, SettingsRepository, WhisperModel};

/// Filename used by [`FileSettingsRepository`] inside an application data dir.
pub const SETTINGS_FILENAME: &str = "settings.json";

/// Persists settings as a pretty-printed JSON file on disk.
///
/// Reads and writes are serialized through an in-process mutex so concurrent
/// load/save calls never interleave on the same file.
pub struct FileSettingsRepository {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSettingsRepository {
    /// A repository that reads and writes settings at `path`.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// A repository that stores its settings file inside `dir`.
    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join(SETTINGS_FILENAME))
    }

    /// The path the settings are stored at.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl SettingsRepository for FileSettingsRepository {
    async fn load(&self) -> Result<AppSettings, CoreError> {
        let _guard = self.lock.lock().await;
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AppSettings::default());
            }
            Err(e) => return Err(CoreError::Io(e)),
        };
        let mut settings: AppSettings = serde_json::from_slice(&bytes)?;
        if settings.whisper_model == WhisperModel::Base {
            settings.whisper_model = WhisperModel::Small;
            let bytes = serde_json::to_vec_pretty(&settings)?;
            tokio::fs::write(&self.path, bytes).await?;
        }
        Ok(settings)
    }

    async fn save(&self, settings: &AppSettings) -> Result<(), CoreError> {
        let _guard = self.lock.lock().await;
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let bytes = serde_json::to_vec_pretty(settings)?;
        tokio::fs::write(&self.path, bytes).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::*;

    /// A unique scratch directory under the system temp directory.
    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("notsai-settings-{name}-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let repo = FileSettingsRepository::in_dir(&dir);
        let settings = AppSettings::default();
        SettingsRepository::save(&repo, &settings).await.unwrap();
        let loaded = SettingsRepository::load(&repo).await.unwrap();
        assert_eq!(loaded, settings);
    }

    #[tokio::test]
    async fn load_upgrades_legacy_base_model_to_small() {
        let dir = temp_dir("model-upgrade");
        let repo = FileSettingsRepository::in_dir(&dir);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(repo.path(), br#"{"whisper_model": "base"}"#)
            .await
            .unwrap();
        let loaded = SettingsRepository::load(&repo).await.unwrap();
        assert_eq!(loaded.whisper_model, WhisperModel::Small);
        let on_disk = tokio::fs::read_to_string(repo.path()).await.unwrap();
        assert!(on_disk.contains("\"whisper_model\": \"small\""));
    }

    #[tokio::test]
    async fn load_missing_file_returns_defaults() {
        let dir = temp_dir("missing");
        let repo = FileSettingsRepository::in_dir(&dir);
        let loaded = SettingsRepository::load(&repo).await.unwrap();
        assert_eq!(loaded, AppSettings::default());
    }

    #[tokio::test]
    async fn load_corrupt_file_returns_json_error() {
        let dir = temp_dir("corrupt");
        let repo = FileSettingsRepository::in_dir(&dir);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(repo.path(), b"{ not json").await.unwrap();
        let err = SettingsRepository::load(&repo).await.unwrap_err();
        assert!(matches!(err, CoreError::Json(_)));
    }

    #[tokio::test]
    async fn save_creates_parent_directories() {
        let dir = temp_dir("nested");
        let repo =
            FileSettingsRepository::new(dir.join("deep").join("path").join(SETTINGS_FILENAME));
        SettingsRepository::save(&repo, &AppSettings::default())
            .await
            .unwrap();
        assert!(repo.path().exists());
    }

    #[tokio::test]
    async fn in_dir_points_at_settings_file() {
        let dir = temp_dir("in-dir");
        let repo = FileSettingsRepository::in_dir(&dir);
        assert_eq!(repo.path(), dir.join(SETTINGS_FILENAME).as_path());
    }

    #[tokio::test]
    async fn save_can_read_back_after_modification() {
        let dir = temp_dir("modified");
        let repo = FileSettingsRepository::in_dir(&dir);
        let settings = AppSettings {
            default_title: "Roadmap sync".to_string(),
            ..Default::default()
        };
        SettingsRepository::save(&repo, &settings).await.unwrap();
        let loaded = SettingsRepository::load(&repo).await.unwrap();
        assert_eq!(loaded.default_title, "Roadmap sync");
    }
}
