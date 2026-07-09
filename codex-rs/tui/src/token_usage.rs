//! TUI token usage models and display formatting.

use std::fmt;

use codex_protocol::num_format::format_with_separators;
use serde::Deserialize;
use serde::Serialize;

const BASELINE_TOKENS: i64 = 12000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl TokenUsage {
    pub fn is_zero(&self) -> bool {
        self.total_tokens == 0
    }

    pub(crate) fn cached_input(&self) -> i64 {
        self.cached_input_tokens.max(0)
    }

    pub(crate) fn non_cached_input(&self) -> i64 {
        (self.input_tokens - self.cached_input()).max(0)
    }

    pub(crate) fn blended_total(&self) -> i64 {
        (self.non_cached_input() + self.output_tokens.max(0)).max(0)
    }

    /// Returns the raw `total_tokens` value. For `last_token_usage`, this is the latest active
    /// context size; for `total_token_usage`, this is the accumulated session total.
    pub(crate) fn tokens_in_context_window(&self) -> i64 {
        self.total_tokens
    }

    pub(crate) fn percent_of_context_window_remaining(&self, context_window: i64) -> i64 {
        if context_window <= BASELINE_TOKENS {
            return 0;
        }
        let effective_window = context_window - BASELINE_TOKENS;
        let used = (self.tokens_in_context_window() - BASELINE_TOKENS).max(0);
        let remaining = (effective_window - used).max(0);
        ((remaining as f64 / effective_window as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as i64
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TokenUsageInfo {
    pub(crate) total_token_usage: TokenUsage,
    pub(crate) last_token_usage: TokenUsage,
    pub(crate) model_context_window: Option<i64>,
}

impl fmt::Display for TokenUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Token usage: total={} input={}{} output={}{}",
            format_with_separators(self.blended_total()),
            format_with_separators(self.non_cached_input()),
            if self.cached_input() > 0 {
                format!(
                    " (+ {} cached)",
                    format_with_separators(self.cached_input())
                )
            } else {
                String::new()
            },
            format_with_separators(self.output_tokens),
            if self.reasoning_output_tokens > 0 {
                format!(
                    " (reasoning {})",
                    format_with_separators(self.reasoning_output_tokens)
                )
            } else {
                String::new()
            }
        )
    }
}

/// Per-million-token pricing for one model family.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    /// Cost per 1M non-cached input tokens.
    input_per_million: f64,
    /// Cost per 1M cached input tokens.
    cached_input_per_million: f64,
    /// Cost per 1M output tokens.
    output_per_million: f64,
    /// Cost per 1M reasoning output tokens.
    reasoning_per_million: f64,
}

/// Looks up approximate pricing for the given model name.
///
/// Falls back to a generic mid-range estimate when the model is not recognised.
fn pricing_for_model(model: &str) -> ModelPricing {
    let m = model.to_lowercase();
    // DeepSeek family (V3/V4) — official platform rates (approximate).
    if m.contains("deepseek") {
        return ModelPricing {
            input_per_million: 0.14,
            cached_input_per_million: 0.014,
            output_per_million: 0.28,
            reasoning_per_million: 0.28,
        };
    }
    // Generic fallback — rough mid-range estimate.
    ModelPricing {
        input_per_million: 1.00,
        cached_input_per_million: 0.10,
        output_per_million: 3.00,
        reasoning_per_million: 3.00,
    }
}

/// Estimates the $ cost of a session's token usage against `model` pricing.
pub(crate) fn estimate_session_cost(usage: &TokenUsage, model: &str) -> f64 {
    let p = pricing_for_model(model);
    let non_cached = usage.non_cached_input().max(0) as f64;
    let cached = usage.cached_input() as f64;
    let output = usage.output_tokens.max(0) as f64;
    let reasoning = usage.reasoning_output_tokens.max(0) as f64;

    (non_cached * p.input_per_million
        + cached * p.cached_input_per_million
        + output * p.output_per_million
        + reasoning * p.reasoning_per_million)
        / 1_000_000.0
}

/// Formats a dollar amount for human-readable display in the status line.
pub(crate) fn format_cost(amount: f64) -> String {
    if amount < 0.01 {
        format!("${:.4}", amount)
    } else if amount < 1.0 {
        format!("${:.3}", amount)
    } else {
        format!("${:.2}", amount)
    }
}
