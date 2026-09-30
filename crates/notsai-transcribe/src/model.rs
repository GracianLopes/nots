//! Static metadata about the whisper.cpp GGML models that this crate can run.

use notsai_core::{SttLanguage, WhisperModel};

/// The default models directory path used by this crate.
pub const MODELS_DIR: &str = "models";

/// The remote base URL for whisper.cpp model files.
const MODELS_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// The file name of the given model on disk.
pub fn model_file_name(model: WhisperModel) -> &'static str {
    match model {
        WhisperModel::Tiny => "ggml-tiny.bin",
        WhisperModel::Base => "ggml-base.bin",
        WhisperModel::Small => "ggml-small.bin",
        WhisperModel::SmallEn => "ggml-small.en.bin",
        WhisperModel::Medium => "ggml-medium.bin",
        WhisperModel::LargeV3 => "ggml-large-v3.bin",
    }
}

/// The URL from which the given model can be downloaded.
pub fn model_download_url(model: WhisperModel) -> String {
    format!("{MODELS_BASE_URL}/{}", model_file_name(model))
}

/// The approximate size of the given model in bytes.
///
/// These values are release-time approximations used only to sanity check a
/// finished download; the exact file size may differ slightly across releases.
pub fn model_size_bytes(model: WhisperModel) -> u64 {
    match model {
        WhisperModel::Tiny => 78_643_200,
        WhisperModel::Base => 148_897_792,
        WhisperModel::Small => 488_636_416,
        WhisperModel::SmallEn => 487_614_201,
        WhisperModel::Medium => 1_610_612_736,
        WhisperModel::LargeV3 => 3_328_599_654,
    }
}

/// The languages supported by the given model.
///
/// All multilingual models cover English, Hindi and Marathi. The English-only
/// small model only covers English.
pub fn model_languages(model: WhisperModel) -> &'static [SttLanguage] {
    match model {
        WhisperModel::SmallEn => &[SttLanguage::En],
        _ => &[SttLanguage::En, SttLanguage::Hi, SttLanguage::Mr],
    }
}

/// The whisper.cpp language code for the given language.
pub fn language_code(language: SttLanguage) -> &'static str {
    match language {
        SttLanguage::En => "en",
        SttLanguage::Hi => "hi",
        SttLanguage::Mr => "mr",
        SttLanguage::Auto => "auto",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_ggml_binaries() {
        assert_eq!(model_file_name(WhisperModel::Tiny), "ggml-tiny.bin");
        assert_eq!(model_file_name(WhisperModel::Base), "ggml-base.bin");
        assert_eq!(model_file_name(WhisperModel::Small), "ggml-small.bin");
        assert_eq!(model_file_name(WhisperModel::SmallEn), "ggml-small.en.bin");
        assert_eq!(model_file_name(WhisperModel::Medium), "ggml-medium.bin");
        assert_eq!(model_file_name(WhisperModel::LargeV3), "ggml-large-v3.bin");
    }

    #[test]
    fn download_urls_point_at_whisper_cpp_releases() {
        let expected = |name: &str| {
            format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{name}")
        };
        assert_eq!(
            model_download_url(WhisperModel::Tiny),
            expected("ggml-tiny.bin")
        );
        assert_eq!(
            model_download_url(WhisperModel::Base),
            expected("ggml-base.bin")
        );
        assert_eq!(
            model_download_url(WhisperModel::Small),
            expected("ggml-small.bin")
        );
        assert_eq!(
            model_download_url(WhisperModel::SmallEn),
            expected("ggml-small.en.bin")
        );
        assert_eq!(
            model_download_url(WhisperModel::Medium),
            expected("ggml-medium.bin")
        );
        assert_eq!(
            model_download_url(WhisperModel::LargeV3),
            expected("ggml-large-v3.bin")
        );
    }

    #[test]
    fn sizes_grow_with_model_quality() {
        let tiny = model_size_bytes(WhisperModel::Tiny);
        let base = model_size_bytes(WhisperModel::Base);
        let small = model_size_bytes(WhisperModel::Small);
        let small_en = model_size_bytes(WhisperModel::SmallEn);
        let medium = model_size_bytes(WhisperModel::Medium);
        let large = model_size_bytes(WhisperModel::LargeV3);
        assert!(
            tiny < base && base < small_en && small_en < small && small < medium && medium < large
        );
    }

    #[test]
    fn multilingual_models_support_all_languages() {
        for model in [
            WhisperModel::Tiny,
            WhisperModel::Base,
            WhisperModel::Small,
            WhisperModel::Medium,
            WhisperModel::LargeV3,
        ] {
            assert_eq!(
                model_languages(model),
                &[SttLanguage::En, SttLanguage::Hi, SttLanguage::Mr]
            );
        }
    }

    #[test]
    fn small_en_supports_only_english() {
        assert_eq!(model_languages(WhisperModel::SmallEn), &[SttLanguage::En]);
    }

    #[test]
    fn language_codes_are_whisper_compatible() {
        assert_eq!(language_code(SttLanguage::En), "en");
        assert_eq!(language_code(SttLanguage::Hi), "hi");
        assert_eq!(language_code(SttLanguage::Mr), "mr");
        assert_eq!(language_code(SttLanguage::Auto), "auto");
    }
}
