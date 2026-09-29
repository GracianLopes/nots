//! User-facing settings for the notsAI application.

use serde::{Deserialize, Serialize};

/// How long raw recording data is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetentionMode {
    /// Keep the raw recording, transcript and notes.
    #[default]
    FullRecording,
    /// Keep the transcript and notes, discard the raw recording.
    SmartNotes,
    /// Keep only the derived notes.
    NotesOnly,
}

/// Where AI processing runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AIMode {
    Cloud,
    /// Local-first: all processing stays on device.
    #[default]
    Local,
    Hybrid,
}

/// Privacy preference derived from the configured AI mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyLevel {
    #[default]
    MaximumPrivacy,
    Balanced,
    MaximumSpeed,
}

impl From<AIMode> for PrivacyLevel {
    fn from(mode: AIMode) -> Self {
        match mode {
            AIMode::Local => PrivacyLevel::MaximumPrivacy,
            AIMode::Hybrid => PrivacyLevel::Balanced,
            AIMode::Cloud => PrivacyLevel::MaximumSpeed,
        }
    }
}

/// Supported AI providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Gemini,
    Groq,
    OpenRouter,
    /// A local OpenAI-compatible endpoint (e.g. Ollama or LM Studio).
    Local,
}

/// Speech-to-text language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SttLanguage {
    #[default]
    En,
    Hi,
    Mr,
    /// Detect the spoken language per chunk and switch between supported
    /// languages mid-transcript (e.g. code-switching English/Hindi/Marathi).
    Auto,
}

/// whisper.cpp model size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhisperModel {
    Tiny,
    Base,
    #[default]
    Small,
    Medium,
    LargeV3,
}

/// Connection details for a single AI provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    /// Optional API key; omitted from serialization when unset and never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Optional custom base URL for OpenAI-compatible local backends
    /// (e.g. Ollama or LM Studio); omitted from serialization when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// The persisted app settings blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub retention: RetentionMode,
    #[serde(default)]
    pub ai_mode: AIMode,
    #[serde(default)]
    pub stt_language: SttLanguage,
    #[serde(default)]
    pub whisper_model: WhisperModel,
    #[serde(default = "default_title")]
    pub default_title: String,
    #[serde(default = "default_providers")]
    pub providers: Vec<ProviderConfig>,
    /// The provider used for AI notes; `None` falls back to the first
    /// configured provider.
    #[serde(default)]
    pub active_provider: Option<ProviderKind>,
    /// User consent for AI notes processing; default off (record-only).
    #[serde(default)]
    pub ai_consent: bool,
}

impl AppSettings {
    /// The provider configuration for the given kind, if present.
    pub fn provider(&self, kind: ProviderKind) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.kind == kind)
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            retention: RetentionMode::default(),
            ai_mode: AIMode::default(),
            stt_language: SttLanguage::default(),
            whisper_model: WhisperModel::default(),
            default_title: default_title(),
            providers: default_providers(),
            active_provider: None,
            ai_consent: false,
        }
    }
}

fn default_title() -> String {
    "New meeting".to_string()
}

fn default_providers() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            kind: ProviderKind::Gemini,
            model: "gemini-2.5-flash".to_string(),
            api_key: None,
            base_url: None,
        },
        ProviderConfig {
            kind: ProviderKind::Groq,
            model: "llama-3.3-70b-versatile".to_string(),
            api_key: None,
            base_url: None,
        },
        ProviderConfig {
            kind: ProviderKind::OpenRouter,
            model: "meta-llama/llama-3.3-70b-instruct".to_string(),
            api_key: None,
            base_url: None,
        },
        ProviderConfig {
            kind: ProviderKind::Local,
            model: "llama3.2".to_string(),
            api_key: None,
            base_url: Some("http://localhost:11434/v1".to_string()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_level_follows_ai_mode() {
        assert_eq!(
            PrivacyLevel::from(AIMode::Local),
            PrivacyLevel::MaximumPrivacy
        );
        assert_eq!(PrivacyLevel::from(AIMode::Hybrid), PrivacyLevel::Balanced);
        assert_eq!(
            PrivacyLevel::from(AIMode::Cloud),
            PrivacyLevel::MaximumSpeed
        );
    }

    #[test]
    fn default_settings_are_local_first() {
        let settings = AppSettings::default();
        assert_eq!(settings.ai_mode, AIMode::Local);
        assert_eq!(
            PrivacyLevel::from(settings.ai_mode),
            PrivacyLevel::MaximumPrivacy
        );
        assert_eq!(settings.whisper_model, WhisperModel::Small);
        assert_eq!(settings.stt_language, SttLanguage::En);
        assert_eq!(settings.retention, RetentionMode::FullRecording);
        assert_eq!(settings.active_provider, None);
        assert!(!settings.ai_consent);

        let models: Vec<&str> = settings
            .providers
            .iter()
            .map(|p| p.model.as_str())
            .collect();
        assert_eq!(
            models,
            vec![
                "gemini-2.5-flash",
                "llama-3.3-70b-versatile",
                "meta-llama/llama-3.3-70b-instruct",
                "llama3.2"
            ]
        );
    }

    #[test]
    fn api_key_is_omitted_when_none() {
        let json = serde_json::to_string(&AppSettings::default()).unwrap();
        assert!(!json.contains("api_key"));
    }

    #[test]
    fn api_key_round_trips_when_present() {
        let mut settings = AppSettings::default();
        settings.providers[0].api_key = Some("secret".to_string());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("secret"));

        let decoded: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.providers[0].api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn provider_lookup_finds_configured_kind() {
        let settings = AppSettings::default();
        assert_eq!(
            settings.provider(ProviderKind::Gemini).unwrap().kind,
            ProviderKind::Gemini
        );
        assert!(settings.provider(ProviderKind::OpenRouter).is_some());
        assert!(settings.provider(ProviderKind::Local).is_some());
    }

    #[test]
    fn base_url_is_omitted_when_none() {
        let cfg = ProviderConfig {
            kind: ProviderKind::Gemini,
            model: "gemini-2.5-flash".to_string(),
            api_key: None,
            base_url: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("base_url"));
        assert!(!json.contains("api_key"));
    }

    #[test]
    fn local_base_url_round_trips() {
        let settings = AppSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("http://localhost:11434/v1"));

        let decoded: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            decoded
                .provider(ProviderKind::Local)
                .unwrap()
                .base_url
                .as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn consent_and_active_provider_round_trip() {
        let settings = AppSettings {
            ai_consent: true,
            active_provider: Some(ProviderKind::Local),
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"ai_consent\":true"));
        assert!(json.contains("\"active_provider\":\"local\""));

        let decoded: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(decoded.ai_consent);
        assert_eq!(decoded.active_provider, Some(ProviderKind::Local));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let decoded: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(decoded, AppSettings::default());
    }
}
