//! Policy gate: free-first, default-deny paid, plus the anti-paid-fallback
//! guard that inspects per-call telemetry after every "free" call.
//!
//! Vendor-neutral: the warning text and free-host policy carry no
//! organization-specific strings. A downstream skin can wrap this gate to add
//! its own messaging (cost portals, manager reports, etc.).

use crate::corpus::ModelEntry;
use url::Url;

pub const PAID_WARNING: &str = "This is a PAID model. Continuing will incur usage \
charges billed to your account. Pass --allow-paid to skip this prompt.";

/// Per-call telemetry consumed by the anti-paid-fallback guard.
///
/// `cost_usd`, `api_base`, and `attempted_fallbacks` come from provider
/// response metadata (e.g. LiteLLM `x-litellm-*` headers). Upstream
/// `ChatResponse` does not surface these yet, so callers fill what they can;
/// the guard degrades gracefully (and `strict_free` fails closed when nothing
/// is available).
#[derive(Debug, Clone, Default)]
pub struct CallTelemetry {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: Option<f64>,
    pub api_base: Option<String>,
    pub attempted_fallbacks: Option<u32>,
    pub latency_ms: u64,
}

/// Extract the lowercased host from an `api_base` value. Handles userinfo,
/// port, and path correctly (no spoofable substring matching). Accepts bare
/// `host[:port]` values by retrying with an `https://` scheme.
fn host_of(api_base: &str) -> Option<String> {
    let parse = |s: &str| {
        Url::parse(s)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
    };
    parse(api_base).or_else(|| parse(&format!("https://{api_base}")))
}

/// True when `host` equals a free host exactly or is a subdomain of one.
fn host_is_free(host: &str, free_hosts: &[String]) -> bool {
    free_hosts.iter().any(|h| {
        let h = h.trim().to_ascii_lowercase();
        !h.is_empty() && (host == h || host.ends_with(&format!(".{h}")))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Free model: proceed.
    Allow,
    /// Paid model: requires interactive confirmation (or --allow-paid).
    NeedConfirm(String),
}

pub struct PolicyGate {
    allow_paid: bool,
    free_hosts: Vec<String>,
    /// Fail closed when free-call telemetry is missing entirely.
    strict_free: bool,
}

impl PolicyGate {
    /// `strict_free` should typically be `cfg.strict_free && !lenient_flag`.
    pub fn new(free_hosts: Vec<String>, allow_paid: bool, strict_free: bool) -> Self {
        Self {
            allow_paid,
            free_hosts,
            strict_free,
        }
    }

    /// Pre-call decision.
    pub fn check(&self, model: &ModelEntry) -> Decision {
        if model.free || self.allow_paid {
            Decision::Allow
        } else {
            let why = model
                .gated_reason
                .clone()
                .unwrap_or_else(|| "paid model".into());
            Decision::NeedConfirm(format!("{PAID_WARNING}\n  model={} ({why})", model.id))
        }
    }

    /// Post-call guard: a model we treated as FREE must have actually been
    /// served from a free backend at $0 with no silent fallback. Catches the
    /// free->paid fallback cost trap that LiteLLM-style proxies can introduce.
    pub fn verify_free(&self, model: &ModelEntry, t: &CallTelemetry) -> Result<(), String> {
        if !model.free {
            return Ok(());
        }
        if let Some(cost) = t.cost_usd {
            if cost > 0.0 {
                return Err(format!(
                    "free model {} billed ${cost} (paid fallback)",
                    model.id
                ));
            }
        }
        if let Some(fb) = t.attempted_fallbacks {
            if fb > 0 {
                return Err(format!("free model {} used {fb} fallback(s)", model.id));
            }
        }
        if let Some(base) = &t.api_base {
            let served_free = host_of(base)
                .map(|host| host_is_free(&host, &self.free_hosts))
                .unwrap_or(false);
            if !served_free {
                return Err(format!(
                    "free model {} served from non-free backend {base}",
                    model.id
                ));
            }
        }
        // Fail closed: with no cost, no api_base, and no fallback signal we
        // cannot prove this call was actually free.
        if self.strict_free
            && t.cost_usd.is_none()
            && t.api_base.is_none()
            && t.attempted_fallbacks.is_none()
        {
            return Err(format!(
                "free model {} returned no cost/api_base/fallback telemetry; cannot \
                 verify it was served free (strict mode; pass --lenient-telemetry to allow)",
                model.id
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            free: true,
            ..Default::default()
        }
    }
    fn paid(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            free: false,
            ..Default::default()
        }
    }

    #[test]
    fn paid_models_need_confirmation_by_default() {
        let gate = PolicyGate::new(vec![], false, false);
        assert!(matches!(gate.check(&free("a")), Decision::Allow));
        assert!(matches!(gate.check(&paid("b")), Decision::NeedConfirm(_)));
    }

    #[test]
    fn allow_paid_overrides_gate() {
        let gate = PolicyGate::new(vec![], true, false);
        assert!(matches!(gate.check(&paid("b")), Decision::Allow));
    }

    #[test]
    fn free_call_billed_is_a_paid_fallback() {
        let gate = PolicyGate::new(vec!["free.example".into()], false, false);
        let tel = CallTelemetry {
            cost_usd: Some(0.02),
            ..Default::default()
        };
        assert!(gate.verify_free(&free("a"), &tel).is_err());
    }

    #[test]
    fn strict_free_fails_closed_without_telemetry() {
        let strict = PolicyGate::new(vec![], false, true);
        assert!(
            strict
                .verify_free(&free("a"), &CallTelemetry::default())
                .is_err()
        );
        let lenient = PolicyGate::new(vec![], false, false);
        assert!(
            lenient
                .verify_free(&free("a"), &CallTelemetry::default())
                .is_ok()
        );
    }
}
