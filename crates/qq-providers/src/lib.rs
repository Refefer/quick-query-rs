//! qq-providers: LLM provider implementations for quick-query
//!
//! This crate provides implementations of the Provider trait for various LLM APIs.

pub mod anthropic;
pub mod context_windows;
pub mod gemini;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;
pub use openai::OpenAIProvider;

/// Check if image content is supported based on the provider's supported_content_types.
pub fn supports_images(supported_types: &Option<Vec<String>>) -> bool {
    match supported_types {
        None => true, // Default: images supported
        Some(types) => types.iter().any(|t| t == "image"),
    }
}

/// Check if content contains any image parts.
pub fn content_has_images(content: &qq_core::Content) -> bool {
    match content {
        qq_core::Content::Text(_) => false,
        qq_core::Content::Parts(parts) => parts
            .iter()
            .any(|p| matches!(p, qq_core::ContentPart::Image { .. })),
    }
}

/// Replace image content with text placeholders for text-only providers.
pub fn strip_unsupported_content(
    content: &qq_core::Content,
    supported_types: &Option<Vec<String>>,
) -> qq_core::Content {
    if supports_images(supported_types) {
        return content.clone();
    }
    match content {
        qq_core::Content::Text(s) => qq_core::Content::Text(s.clone()),
        qq_core::Content::Parts(parts) => {
            let stripped: Vec<qq_core::ContentPart> = parts
                .iter()
                .map(|p| match p {
                    qq_core::ContentPart::Image { image } => qq_core::ContentPart::Text {
                        text: format!(
                            "[Image: {}, {}x{}]",
                            image.media_type, image.width, image.height
                        ),
                    },
                    other => other.clone(),
                })
                .collect();
            qq_core::Content::Parts(stripped)
        }
    }
}
