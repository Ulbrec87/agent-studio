//! Detect the active model from terminal output (Hydra `shared/terminalModelDetection.ts`).

use std::sync::LazyLock;

use regex::Regex;

use crate::types::{ModelId, ProviderId, provider_model_ids};

static ANSI_ESCAPE: LazyLock<Regex> = LazyLock::new(|| {
    // Same pattern as Hydra: a 2-char escape, or a CSI sequence.
    Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").expect("static regex")
});
static CODEX_MODEL_CHANGED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)Model changed to\s+([A-Za-z0-9._-]+)").expect("static regex")
});
static CODEX_CURRENT_MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)Current model[:\s]+([A-Za-z0-9._-]+)").expect("static regex")
});
static CODEX_USING_MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)Using model[:\s]+([A-Za-z0-9._-]+)").expect("static regex")
});
static CLAUDE_FAMILY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)(haiku|sonnet|opus)(?-u:\b)").expect("static regex"));
static GPT_MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^gpt-[a-z0-9.-]+(?:-codex(?:-(?:mini|max))?)?$").expect("static regex")
});

pub fn strip_ansi(text: &str) -> String {
    ANSI_ESCAPE.replace_all(text, "").into_owned()
}

fn last_match(text: &str, pattern: &Regex) -> Option<String> {
    pattern
        .captures_iter(text)
        .filter_map(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .last()
        .filter(|s| !s.is_empty())
}

/// The model the terminal output last announced, normalized to a model id, if any.
pub fn detect_latest_model_from_terminal_output(
    provider: ProviderId,
    text: &str,
) -> Option<ModelId> {
    let cleaned = strip_ansi(text);
    if cleaned.is_empty() {
        return None;
    }
    let found = match provider {
        ProviderId::Codex => last_match(&cleaned, &CODEX_MODEL_CHANGED)
            .or_else(|| last_match(&cleaned, &CODEX_CURRENT_MODEL))
            .or_else(|| last_match(&cleaned, &CODEX_USING_MODEL)),
        _ => last_match(&cleaned, &CLAUDE_FAMILY),
    };
    normalize_detected_model(provider, found)
}

fn normalize_detected_model(provider: ProviderId, value: Option<String>) -> Option<ModelId> {
    let normalized = value?.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }
    if provider_model_ids(provider).any(|id| id.to_lowercase() == normalized) {
        return Some(normalized);
    }
    if provider == ProviderId::Claude {
        return ["opus", "sonnet", "haiku"]
            .into_iter()
            .find(|family| normalized.contains(family))
            .map(str::to_string);
    }
    if GPT_MODEL.is_match(&normalized) {
        return Some(normalized);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_codex_model_changes_from_terminal_output() {
        assert_eq!(
            detect_latest_model_from_terminal_output(
                ProviderId::Codex,
                "Model changed to gpt-5.4 high"
            )
            .as_deref(),
            Some("gpt-5.4")
        );
    }

    #[test]
    fn ignores_unrelated_claude_terminal_text() {
        assert_eq!(
            detect_latest_model_from_terminal_output(ProviderId::Claude, "Checking for updates..."),
            None
        );
    }

    #[test]
    fn maps_claude_family_names_to_model_ids() {
        assert_eq!(
            detect_latest_model_from_terminal_output(
                ProviderId::Claude,
                "Opus 4.6 | session stats"
            )
            .as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn strips_ansi_and_takes_the_last_mention() {
        let text = "\x1b[1mSonnet\x1b[0m ... \x1b[32mUsing model: gpt-5.2-codex\x1b[0m";
        assert_eq!(
            detect_latest_model_from_terminal_output(ProviderId::Claude, text).as_deref(),
            Some("sonnet")
        );
        assert_eq!(
            detect_latest_model_from_terminal_output(ProviderId::Codex, text).as_deref(),
            Some("gpt-5.2-codex")
        );
        assert_eq!(
            detect_latest_model_from_terminal_output(ProviderId::Claude, "haiku then \x1b[2Kopus")
                .as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn rejects_unknown_codex_models() {
        assert_eq!(
            detect_latest_model_from_terminal_output(ProviderId::Codex, "Model changed to banana"),
            None
        );
        assert_eq!(
            detect_latest_model_from_terminal_output(
                ProviderId::Codex,
                "Model changed to GPT-9-codex-max"
            )
            .as_deref(),
            Some("gpt-9-codex-max")
        );
    }
}
