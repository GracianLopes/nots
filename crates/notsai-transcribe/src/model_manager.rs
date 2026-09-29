//! Downloads whisper.cpp GGML model files into the app data directory.
//!
//! Model files are treated as user data and never regenerated implicitly: a
//! cached model is used as-is without re-downloading, and any fresh download is
//! gated behind explicit user consent.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notsai_core::{CoreError, WhisperModel};
use tokio::io::AsyncWriteExt;

use crate::model::{model_download_url, model_file_name, model_size_bytes};

/// How long a model download may take before it is abandoned.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Whether the user has explicitly agreed to download a speech model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// The user (or operator) has agreed to download the model.
    Granted,
    /// The user has not agreed to download the model.
    NotGranted,
}

/// Manages the local presence of whisper.cpp model files.
#[derive(Debug)]
pub struct ModelManager {
    models_dir: PathBuf,
    client: reqwest::Client,
}

impl ModelManager {
    /// Creates a manager that downloads model files into `models_dir`.
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
            client: reqwest::Client::new(),
        }
    }

    /// The directory this manager downloads model files into.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// The on-disk path for `model` if it is already cached and non-empty.
    pub fn cached_path(&self, model: WhisperModel) -> Option<PathBuf> {
        let path = self.models_dir.join(model_file_name(model));
        std::fs::metadata(&path)
            .ok()
            .filter(|meta| meta.len() > 0)
            .map(|_| path)
    }

    /// Returns the path to `model`, downloading it first when required.
    ///
    /// A cached model is returned without consulting `consent`. Otherwise the
    /// download is only performed when [`Consent::Granted`] was provided. The
    /// download is streamed to a temporary `.part` file and atomically renamed
    /// into place once its size matches the expected model size. `progress` is
    /// invoked with `(downloaded, total)` bytes as the download advances.
    pub async fn ensure(
        &self,
        model: WhisperModel,
        consent: Consent,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<PathBuf, CoreError> {
        if let Some(path) = self.cached_path(model) {
            return Ok(path);
        }

        match consent {
            Consent::Granted => {}
            Consent::NotGranted => {
                return Err(CoreError::transcription(
                    "downloading the speech model requires explicit user consent",
                ));
            }
        }

        self.download(model, progress).await
    }

    /// Downloads `model` into the models directory.
    ///
    /// Any pre-existing `.part` file is treated as an interrupted download and
    /// resumed from its current size. The completed file is atomically renamed
    /// into place once its size matches the expected model size. `progress` is
    /// invoked with `(downloaded, total)` bytes as the download advances.
    async fn download(
        &self,
        model: WhisperModel,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<PathBuf, CoreError> {
        let file_name = model_file_name(model);
        let url = model_download_url(model);
        let target = self.models_dir.join(file_name);
        let temp = target.with_file_name(format!("{file_name}.part"));

        tokio::fs::create_dir_all(&self.models_dir).await?;

        let expected = model_size_bytes(model);
        let offset = Self::resume_offset(
            tokio::fs::metadata(&temp).await.ok().map(|meta| meta.len()),
            expected,
        );
        if offset == expected {
            return Self::promote(&temp, &target, file_name, expected, offset).await;
        }

        let mut request = self.client.get(&url).timeout(DOWNLOAD_TIMEOUT);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().await.map_err(|error| {
            CoreError::transcription(format!(
                "failed to start model download from {url}: {error}"
            ))
        })?;
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut response = response.error_for_status().map_err(|error| {
            CoreError::transcription(format!("model download from {url} failed: {error}"))
        })?;

        let mut file = if resumed {
            tokio::fs::OpenOptions::new()
                .write(true)
                .append(true)
                .open(&temp)
                .await?
        } else {
            tokio::fs::File::create(&temp).await?
        };
        let resume_base = if resumed { offset } else { 0 };
        let total = resume_base.saturating_add(response.content_length().unwrap_or(expected));
        let mut downloaded = resume_base;
        let mut last_percent = -1;
        Self::report_progress(progress, &mut last_percent, downloaded, total, true);
        loop {
            let chunk = response.chunk().await.map_err(|error| {
                CoreError::transcription(format!(
                    "model download from {url} was interrupted: {error}"
                ))
            })?;
            match chunk {
                Some(bytes) => {
                    file.write_all(&bytes).await?;
                    downloaded = downloaded.saturating_add(bytes.len() as u64);
                    Self::report_progress(progress, &mut last_percent, downloaded, total, false);
                }
                None => break,
            }
        }
        file.flush().await?;
        Self::report_progress(progress, &mut last_percent, downloaded, total, true);

        let actual = file.metadata().await?.len();
        drop(file);
        Self::promote(&temp, &target, file_name, expected, actual).await
    }

    /// Whether `actual` is close enough to the expected model size to trust it.
    ///
    /// Expected sizes are release-time approximations, so a finished download
    /// is accepted within a 5% tolerance on either side.
    fn size_matches_expectation(actual: u64, expected: u64) -> bool {
        actual >= expected * 95 / 100 && actual <= expected * 105 / 100
    }

    /// The byte offset to resume a download from, given the size of the `.part`
    /// file left on disk.
    ///
    /// Returns `0` when there is nothing to resume (no file) or the existing
    /// file is too large to belong to this model.
    fn resume_offset(existing_len: Option<u64>, expected: u64) -> u64 {
        match existing_len {
            Some(len) if len > 0 && len <= expected * 105 / 100 => len,
            _ => 0,
        }
    }

    /// Validates a finished download and atomically moves it into place.
    ///
    /// On a size mismatch the `.part` file is discarded and an error returned,
    /// so a corrupt download can be retried from scratch.
    async fn promote(
        temp: &Path,
        target: &Path,
        file_name: &str,
        expected: u64,
        actual: u64,
    ) -> Result<PathBuf, CoreError> {
        if !Self::size_matches_expectation(actual, expected) {
            let _ = tokio::fs::remove_file(temp).await;
            return Err(CoreError::transcription(format!(
                "downloaded {file_name} was {actual} bytes; expected about {expected} bytes"
            )));
        }
        tokio::fs::rename(temp, target).await?;
        Ok(target.to_path_buf())
    }

    /// Calls `progress` at most once per full percentage point, plus a forced
    /// final call so the end of the download is always reported.
    fn report_progress(
        progress: &mut (dyn FnMut(u64, u64) + Send),
        last_percent: &mut i64,
        downloaded: u64,
        total: u64,
        force: bool,
    ) {
        let percent = (downloaded
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)) as i64;
        if force || percent != *last_percent {
            *last_percent = percent;
            progress(downloaded, total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("notsai-model-manager-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn cached_path_is_none_before_file_exists() {
        let dir = temp_dir();
        let manager = ModelManager::new(&dir);
        assert!(manager.cached_path(WhisperModel::Tiny).is_none());
    }

    #[test]
    fn cached_path_is_some_after_file_created() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(model_file_name(WhisperModel::Tiny));
        std::fs::write(&path, b"model").unwrap();

        let manager = ModelManager::new(&dir);
        assert_eq!(manager.cached_path(WhisperModel::Tiny), Some(path));
    }

    #[test]
    fn cached_path_ignores_empty_files() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(model_file_name(WhisperModel::Base));
        std::fs::File::create(&path).unwrap();

        let manager = ModelManager::new(&dir);
        assert!(manager.cached_path(WhisperModel::Base).is_none());
    }

    #[tokio::test]
    async fn ensure_requires_consent_when_not_cached() {
        let dir = temp_dir();
        let manager = ModelManager::new(&dir);

        let error = manager
            .ensure(WhisperModel::Tiny, Consent::NotGranted, &mut |_, _| {})
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("explicit user consent"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn ensure_returns_cached_path_without_consent() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let expected = dir.join(model_file_name(WhisperModel::Tiny));
        std::fs::write(&expected, b"model").unwrap();

        let manager = ModelManager::new(&dir);
        let actual = manager
            .ensure(WhisperModel::Tiny, Consent::NotGranted, &mut |_, _| {})
            .await
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn size_matches_expectation_accepts_near_sizes() {
        let expected = 100_000;
        assert!(ModelManager::size_matches_expectation(expected, expected));
        assert!(ModelManager::size_matches_expectation(
            expected * 95 / 100,
            expected
        ));
        assert!(ModelManager::size_matches_expectation(
            expected * 105 / 100,
            expected
        ));
        assert!(ModelManager::size_matches_expectation(
            expected * 97 / 100,
            expected
        ));
        assert!(ModelManager::size_matches_expectation(
            expected * 103 / 100,
            expected
        ));
    }

    #[test]
    fn size_matches_expectation_rejects_distant_sizes() {
        let expected = 100_000;
        assert!(!ModelManager::size_matches_expectation(
            expected * 94 / 100,
            expected
        ));
        assert!(!ModelManager::size_matches_expectation(
            expected * 106 / 100,
            expected
        ));
        assert!(!ModelManager::size_matches_expectation(0, expected));
        assert!(!ModelManager::size_matches_expectation(u64::MAX, expected));
    }

    #[test]
    fn resume_offset_is_zero_without_existing_file() {
        assert_eq!(ModelManager::resume_offset(None, 100_000), 0);
    }

    #[test]
    fn resume_offset_is_zero_fortoo_large_existing_file() {
        assert_eq!(ModelManager::resume_offset(Some(200_000), 100_000), 0);
    }

    #[test]
    fn resume_offset_reuses_progress_in_tolerance() {
        assert_eq!(ModelManager::resume_offset(Some(40_000), 100_000), 40_000);
        assert_eq!(
            ModelManager::resume_offset(Some(100_000 * 105 / 100), 100_000),
            100_000 * 105 / 100
        );
    }

    #[test]
    fn resume_offset_is_zero_for_empty_file() {
        assert_eq!(ModelManager::resume_offset(Some(0), 100_000), 0);
    }
}
