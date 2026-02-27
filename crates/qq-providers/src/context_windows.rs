/// Look up the known context window size (in tokens) for a model by name.
///
/// Uses prefix matching on the lowercased model name for forward-compatibility
/// with date-suffixed variants (e.g., `claude-sonnet-4-20250514`).
///
/// Returns `None` for unknown models.
pub fn known_context_window(model: &str) -> Option<u32> {
    let model = model.to_lowercase();

    // Claude models — all 200K
    if model.starts_with("claude-") {
        return Some(200_000);
    }

    // Gemini models
    if model.starts_with("gemini-2.5-") || model.starts_with("gemini-2.0-") {
        return Some(1_000_000);
    }
    if model.starts_with("gemini-1.5-pro") {
        return Some(2_000_000);
    }
    if model.starts_with("gemini-1.5-flash") {
        return Some(1_000_000);
    }

    // OpenAI o-series reasoning models
    if model.starts_with("o4-mini") {
        return Some(200_000);
    }
    if model.starts_with("o3-mini") || model.starts_with("o3") {
        return Some(200_000);
    }
    if model.starts_with("o1-mini") {
        return Some(128_000);
    }
    if model == "o1" || model.starts_with("o1-2") {
        return Some(200_000);
    }

    // GPT-4o family
    if model.starts_with("gpt-4o") || model.starts_with("chatgpt-4o") {
        return Some(128_000);
    }

    // GPT-4 Turbo
    if model.starts_with("gpt-4-turbo") {
        return Some(128_000);
    }

    // GPT-4 base (not turbo)
    if model == "gpt-4" || model.starts_with("gpt-4-0") {
        return Some(8_192);
    }

    // GPT-3.5 Turbo
    if model.starts_with("gpt-3.5-turbo") {
        return Some(16_385);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_models() {
        assert_eq!(known_context_window("claude-sonnet-4-20250514"), Some(200_000));
        assert_eq!(known_context_window("claude-opus-4-20250514"), Some(200_000));
        assert_eq!(known_context_window("claude-haiku-3-5-20241022"), Some(200_000));
        assert_eq!(known_context_window("claude-3-opus-20240229"), Some(200_000));
    }

    #[test]
    fn test_gemini_models() {
        assert_eq!(known_context_window("gemini-2.5-pro"), Some(1_000_000));
        assert_eq!(known_context_window("gemini-2.5-flash"), Some(1_000_000));
        assert_eq!(known_context_window("gemini-2.0-flash"), Some(1_000_000));
        assert_eq!(known_context_window("gemini-1.5-pro-latest"), Some(2_000_000));
        assert_eq!(known_context_window("gemini-1.5-flash-latest"), Some(1_000_000));
    }

    #[test]
    fn test_gpt_models() {
        assert_eq!(known_context_window("gpt-4o"), Some(128_000));
        assert_eq!(known_context_window("gpt-4o-2024-08-06"), Some(128_000));
        assert_eq!(known_context_window("gpt-4o-mini"), Some(128_000));
        assert_eq!(known_context_window("chatgpt-4o-latest"), Some(128_000));
        assert_eq!(known_context_window("gpt-4-turbo"), Some(128_000));
        assert_eq!(known_context_window("gpt-4-turbo-2024-04-09"), Some(128_000));
        assert_eq!(known_context_window("gpt-4"), Some(8_192));
        assert_eq!(known_context_window("gpt-4-0613"), Some(8_192));
        assert_eq!(known_context_window("gpt-3.5-turbo"), Some(16_385));
        assert_eq!(known_context_window("gpt-3.5-turbo-0125"), Some(16_385));
    }

    #[test]
    fn test_o_series_models() {
        assert_eq!(known_context_window("o1"), Some(200_000));
        assert_eq!(known_context_window("o1-mini"), Some(128_000));
        assert_eq!(known_context_window("o1-mini-2024-09-12"), Some(128_000));
        assert_eq!(known_context_window("o3"), Some(200_000));
        assert_eq!(known_context_window("o3-mini"), Some(200_000));
        assert_eq!(known_context_window("o4-mini"), Some(200_000));
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(known_context_window("Claude-Sonnet-4-20250514"), Some(200_000));
        assert_eq!(known_context_window("GPT-4o"), Some(128_000));
    }

    #[test]
    fn test_unknown_models() {
        assert_eq!(known_context_window("llama-3.1-70b"), None);
        assert_eq!(known_context_window("mixtral-8x7b"), None);
        assert_eq!(known_context_window("custom-model"), None);
    }
}
