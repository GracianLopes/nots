//! notsai-ai — incremental meeting summarization behind an `AIProvider`
//! trait with pluggable backends: Google Gemini, Groq, OpenRouter and a
//! local endpoint. Providers never receive more than the current rolling
//! context to keep notes aligned with the "understand, don't dump"
//! principle, and no transcript is ever sent without explicit consent.

#![forbid(unsafe_code)]

mod provider;
mod summarizer;

/// Factory that builds a provider from a saved [`ProviderConfig`]-style
/// configuration (see `notsai_core::settings`).
pub use provider::provider_from_config;

/// Rolling-context transcript summarizer that produces structured notes.
pub use summarizer::Summarizer;
