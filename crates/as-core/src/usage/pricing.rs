//! Price table and cost estimate (Hydra `shared/usage/pricing.ts`). The numbers live in [`SPECS`]
//! and nowhere else; bump [`PRICING_VERSION`] when they change so stored costs are recomputed.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::types::TokenCounts;

/// Bump when the table changes, so stored costs get recomputed.
pub const PRICING_VERSION: u32 = 1;

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatPrice {
    pub input: f64,
    pub output: f64,
    pub cache_write5m: f64,
    pub cache_write1h: f64,
    pub cache_read: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub flat: FlatPrice,
    /// Prices that replace `flat` for calls with a prompt larger than `above` tokens.
    pub long_prompt: Option<(u64, FlatPrice)>,
}

impl ModelPrice {
    pub fn input(&self) -> f64 {
        self.flat.input
    }
    pub fn output(&self) -> f64 {
        self.flat.output
    }
}

struct Spec {
    id: &'static str,
    input: f64,
    output: f64,
    /// Absolute cache read price; defaults to 0.1x input.
    cache_read: Option<f64>,
    /// OpenAI models have no cache writes.
    no_cache_write: bool,
    /// `(above, input, output)`.
    long_prompt: Option<(u64, f64, f64)>,
}

const fn claude(id: &'static str, input: f64, output: f64) -> Spec {
    Spec {
        id,
        input,
        output,
        cache_read: None,
        no_cache_write: false,
        long_prompt: None,
    }
}

const fn claude_cr(id: &'static str, input: f64, output: f64, cache_read: f64) -> Spec {
    Spec {
        id,
        input,
        output,
        cache_read: Some(cache_read),
        no_cache_write: false,
        long_prompt: None,
    }
}

const fn openai(id: &'static str, input: f64, output: f64) -> Spec {
    Spec {
        id,
        input,
        output,
        cache_read: None,
        no_cache_write: true,
        long_prompt: None,
    }
}

/// The price table: USD per million tokens (Hydra's numbers).
const SPECS: &[Spec] = &[
    claude_cr("claude-fable-5-1", 10.0, 50.0, 0.25),
    claude_cr("claude-mythos-5-1", 10.0, 50.0, 0.25),
    claude_cr("claude-fable-5", 10.0, 50.0, 1.0),
    claude_cr("claude-mythos-5", 10.0, 50.0, 1.0),
    claude_cr("claude-opus-5-5", 4.0, 20.0, 0.2),
    claude("claude-opus-5", 5.0, 25.0),
    claude("claude-opus-4-8", 5.0, 25.0),
    claude("claude-opus-4-7", 5.0, 25.0),
    claude("claude-opus-4-6", 5.0, 25.0),
    claude("claude-opus-4-5", 5.0, 25.0),
    claude("claude-opus-4-1", 15.0, 75.0),
    claude("claude-opus-4", 15.0, 75.0),
    claude_cr("claude-sonnet-5-5", 2.0, 10.0, 0.2),
    claude_cr("claude-sonnet-5", 2.0, 10.0, 0.2),
    claude("claude-sonnet-4-6", 3.0, 15.0),
    claude("claude-sonnet-4-5", 3.0, 15.0),
    claude("claude-sonnet-4", 3.0, 15.0),
    claude("claude-3-7-sonnet", 3.0, 15.0),
    Spec {
        id: "claude-haiku-5-5",
        input: 0.1,
        output: 0.5,
        cache_read: None,
        no_cache_write: false,
        long_prompt: Some((100_000, 0.5, 2.5)),
    },
    claude("claude-haiku-4-5", 1.0, 5.0),
    claude("claude-3-5-haiku", 0.8, 4.0),
    openai("gpt-5", 1.25, 10.0),
    openai("gpt-5-codex", 1.25, 10.0),
    openai("gpt-5-mini", 0.25, 2.0),
];

fn flat_price(spec: &Spec, input: f64, output: f64) -> FlatPrice {
    FlatPrice {
        input,
        output,
        cache_write5m: if spec.no_cache_write {
            0.0
        } else {
            input * 1.25
        },
        cache_write1h: if spec.no_cache_write {
            0.0
        } else {
            input * 2.0
        },
        cache_read: spec.cache_read.unwrap_or(input * 0.1),
    }
}

fn price_of(spec: &Spec) -> ModelPrice {
    ModelPrice {
        flat: flat_price(spec, spec.input, spec.output),
        long_prompt: spec
            .long_prompt
            .map(|(above, input, output)| (above, flat_price(spec, input, output))),
    }
}

static PRICES: LazyLock<HashMap<&'static str, ModelPrice>> =
    LazyLock::new(|| SPECS.iter().map(|s| (s.id, price_of(s))).collect());

/// Table keys, longest first, so the first prefix hit is the most specific one.
static KEYS_LONGEST_FIRST: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut keys: Vec<&'static str> = SPECS.iter().map(|s| s.id).collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    keys
});

/// Canonical model id: lower case, no provider prefix (`anthropic/`), no context tag (`[1m]`),
/// no date suffix (`-20250929`). Claude's dotted versions become dashed; OpenAI ids such as
/// `gpt-5.1` keep their dot.
pub fn normalize_model(raw: &str) -> String {
    let mut id = raw.trim().to_lowercase();
    // `^[a-z0-9_-]+/`
    if let Some(slash) = id.find('/')
        && slash > 0
        && id[..slash]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        id = id[slash + 1..].to_string();
    }
    // `\[[^\]]*\]$`
    if id.ends_with(']')
        && let Some(open) = id.rfind('[')
        && !id[open + 1..id.len() - 1].contains(']')
    {
        id.truncate(open);
    }
    // `-\d{8}$`
    if id.len() > 9 {
        let (head, tail) = id.split_at(id.len() - 9);
        if tail.starts_with('-') && tail[1..].chars().all(|c| c.is_ascii_digit()) {
            id = head.to_string();
        }
    }
    if id.starts_with("claude") {
        id = id.replace('.', "-");
    }
    id
}

/// Price of a model, `None` when unknown. Exact match first, then (Claude ids only) the longest
/// table key followed by `-`. OpenAI ids match exactly: `gpt-5-codex-mini` is a cheaper model, not
/// a `gpt-5-codex` variant.
pub fn price_for(model: &str) -> Option<ModelPrice> {
    let id = normalize_model(model);
    if let Some(p) = PRICES.get(id.as_str()) {
        return Some(*p);
    }
    if !id.starts_with("claude") {
        return None;
    }
    KEYS_LONGEST_FIRST
        .iter()
        .find(|key| id.strip_prefix(**key).is_some_and(|r| r.starts_with('-')))
        .and_then(|key| PRICES.get(key).copied())
}

/// `<synthetic>` marks records Claude Code writes itself, not model calls.
pub fn is_synthetic_model(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("<synthetic>")
}

/// Estimated API-equivalent cost of a single call, `None` when the model has no price. Per call
/// because some tiers depend on that call's prompt size.
pub fn estimate_cost(model: &str, tokens: &TokenCounts) -> Option<f64> {
    let base = price_for(model)?;
    let prompt = tokens.input + tokens.cache_write5m + tokens.cache_write1h + tokens.cache_read;
    let price = match base.long_prompt {
        Some((above, tier)) if prompt > above => tier,
        _ => base.flat,
    };
    let usd = (tokens.input as f64 * price.input
        + tokens.output as f64 * price.output
        + tokens.cache_write5m as f64 * price.cache_write5m
        + tokens.cache_write1h as f64 * price.cache_write1h
        + tokens.cache_read as f64 * price.cache_read)
        / 1_000_000.0;
    usd.is_finite().then_some(usd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(f: impl FnOnce(&mut TokenCounts)) -> TokenCounts {
        let mut t = TokenCounts::default();
        f(&mut t);
        t
    }
    const M: u64 = 1_000_000;
    fn close(a: Option<f64>, b: f64) {
        let a = a.expect("priced");
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }

    #[test]
    fn normalize_model_cases() {
        assert_eq!(normalize_model("  Claude-Opus-4-5 "), "claude-opus-4-5");
        assert_eq!(
            normalize_model("anthropic/claude-opus-4-5"),
            "claude-opus-4-5"
        );
        assert_eq!(normalize_model("openai/gpt-5"), "gpt-5");
        assert_eq!(
            normalize_model("claude-sonnet-4-5[1m]"),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model("claude-sonnet-4-5-20250929[1m]"),
            "claude-sonnet-4-5"
        );
        assert_eq!(normalize_model("claude-opus-4.5"), "claude-opus-4-5");
        assert_eq!(normalize_model("gpt-5.1"), "gpt-5.1");
    }

    #[test]
    fn price_for_cases() {
        let p = price_for("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!((p.input(), p.output()), (3.0, 15.0));
        assert_eq!(price_for("claude-opus-4-1-foo").unwrap().input(), 15.0);
        assert_eq!(price_for("claude-opus-4-5-foo").unwrap().input(), 5.0);
        // falls back to claude-opus-4
        assert_eq!(price_for("claude-opus-4-9").unwrap().input(), 15.0);
        assert!(price_for("gpt-50").is_none());
        assert!(price_for("gpt-5.1").is_none());
        assert!(price_for("claude-opus-45").is_none());
        assert_eq!(price_for("gpt-5-mini").unwrap().input(), 0.25);
        assert!(price_for("gpt-5-codex-mini").is_none());
        assert!(price_for("mystery-1").is_none());
        assert!(price_for("").is_none());
        assert_eq!(PRICING_VERSION, 1);
    }

    #[test]
    fn estimate_cost_cases() {
        close(
            estimate_cost(
                "claude-sonnet-4-5",
                &tok(|t| {
                    t.input = M;
                    t.output = M
                }),
            ),
            18.0,
        );
        close(
            estimate_cost("claude-sonnet-4-5", &tok(|t| t.cache_write5m = M)),
            3.75,
        );
        close(
            estimate_cost("claude-sonnet-4-5", &tok(|t| t.cache_write1h = M)),
            6.0,
        );
        close(
            estimate_cost("claude-opus-4-5", &tok(|t| t.cache_read = M)),
            0.5,
        );
        close(
            estimate_cost("claude-opus-5-5", &tok(|t| t.cache_read = M)),
            0.2,
        );
        close(
            estimate_cost("claude-fable-5", &tok(|t| t.cache_read = M)),
            1.0,
        );
        close(
            estimate_cost("claude-fable-5-1", &tok(|t| t.cache_read = M)),
            0.25,
        );
    }

    #[test]
    fn haiku_5_5_long_prompt_tier() {
        close(
            estimate_cost("claude-haiku-5-5", &tok(|t| t.input = 100_000)),
            0.01,
        );
        close(
            estimate_cost("claude-haiku-5-5", &tok(|t| t.input = 100_001)),
            0.0500005,
        );
        close(
            estimate_cost(
                "claude-haiku-5-5",
                &tok(|t| {
                    t.input = 1;
                    t.cache_read = 100_000
                }),
            ),
            100_000.0 * 0.5 * 0.1 / M as f64 + 0.5 / M as f64,
        );
        close(
            estimate_cost("claude-haiku-5-5", &tok(|t| t.cache_read = 100_000)),
            0.001,
        );
        close(
            estimate_cost("claude-haiku-5-5", &tok(|t| t.output = 200_000)),
            0.1,
        );
    }

    #[test]
    fn openai_has_no_cache_writes_and_unpriced_is_none() {
        close(
            estimate_cost(
                "gpt-5",
                &tok(|t| {
                    t.input = M;
                    t.output = M;
                    t.cache_read = M
                }),
            ),
            1.25 + 10.0 + 0.125,
        );
        close(estimate_cost("gpt-5", &tok(|t| t.cache_write5m = M)), 0.0);
        assert!(estimate_cost("mystery", &tok(|t| t.input = 5)).is_none());
    }

    #[test]
    fn synthetic_model() {
        assert!(is_synthetic_model("<synthetic>"));
        assert!(!is_synthetic_model("claude-opus-5"));
    }
}
