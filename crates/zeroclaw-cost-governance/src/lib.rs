//! Cost governance for ZeroClaw: free-first model selection, a default-deny
//! paid-spend policy, and a classified model corpus.
//!
//! This crate is purely additive. It does not track or store spend — that is
//! `zeroclaw_config::cost::CostTracker`'s job, and callers feed it
//! `record_usage_with_agent` after each call. What this crate adds is the
//! *selection and policy* layer ZeroClaw lacks:
//!
//! - [`corpus`]: a classified model catalog (free/paid, arena capability ELO,
//!   benched latency) reduced to a single `agentic_score` the router sorts on.
//! - [`router`]: free-first routing by tier (fast / strong / auto) with a
//!   cross-family fallback chain, skipping unhealthy models.
//! - [`policy`]: a default-deny-paid gate plus an anti-paid-fallback guard that
//!   inspects per-call telemetry so a "free" model can't silently bill you.
//! - [`health`]: a per-model circuit-breaker / latency signal used as routing
//!   capability input (not a duplicate of provider-internal reliability state).
//!
//! [`build_lineup`] turns the free corpus + health into a ranked panel that the
//! consensus reviewer can fan out to.

pub mod corpus;
pub mod health;
pub mod policy;
pub mod router;

pub use corpus::{Corpus, ModelEntry, RefreshReport};
pub use health::{HealthStore, ModelHealth, State};
pub use policy::{CallTelemetry, Decision, PAID_WARNING, PolicyGate};
pub use router::{Route, Router, Tier};

/// A ranked panel member: a model id with the inputs to a health-weighted
/// score. Field-compatible with `zeroclaw_consensus::Panelist`; the consumer
/// maps across so this crate stays independent of the consensus crate.
#[derive(Debug, Clone)]
pub struct RankedModel {
    pub model: String,
    pub base_weight: f64,
    pub success_rate: f64,
}

fn success_rate(health: &HealthStore, model: &str) -> f64 {
    match health.models.get(model) {
        Some(h) if h.calls > 0 => {
            let ok = h.calls.saturating_sub(h.failures) as f64;
            (ok / h.calls as f64).clamp(0.0, 1.0)
        }
        _ => 1.0,
    }
}

/// Build a free-first panel of up to `n` models, highest capability first,
/// skipping open circuit breakers. `base_weight` is the model's agentic score
/// normalized against the strongest in the lineup (clamped to [0.3, 1.0]).
pub fn build_lineup(corpus: &Corpus, health: &HealthStore, n: usize) -> Vec<RankedModel> {
    let mut scored: Vec<(&ModelEntry, f64)> = corpus
        .free_chat()
        .filter(|m| !health.breaker_open(&m.id))
        .map(|m| (m, m.agentic_score.or(m.w_swe).unwrap_or(0.0)))
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(n.max(1));

    let max_score = scored.first().map(|(_, s)| *s).unwrap_or(0.0);
    scored
        .into_iter()
        .map(|(m, s)| RankedModel {
            model: m.id.clone(),
            base_weight: if max_score > 0.0 {
                (s / max_score).clamp(0.3, 1.0)
            } else {
                0.7
            },
            success_rate: success_rate(health, &m.id),
        })
        .collect()
}

/// Build a panel from an explicit list of model ids (e.g. user-pinned models).
pub fn lineup_from_models(
    models: &[String],
    corpus: &Corpus,
    health: &HealthStore,
) -> Vec<RankedModel> {
    models
        .iter()
        .map(|id| {
            let base = corpus
                .get(id)
                .and_then(|m| m.agentic_score.or(m.w_swe))
                .map(|s| (s / 1000.0).clamp(0.3, 1.0))
                .unwrap_or(0.7);
            RankedModel {
                model: id.clone(),
                base_weight: base,
                success_rate: success_rate(health, id),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::ModelEntry;

    fn corpus_with(models: Vec<ModelEntry>) -> Corpus {
        Corpus {
            source: "test".into(),
            arena_date: "2026-01-01".into(),
            count: models.len(),
            models,
        }
    }

    fn free_model(id: &str, score: f64) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            family: id.split('/').next().unwrap_or(id).into(),
            kind: "chat".into(),
            route_candidate: true,
            free: true,
            agentic_score: Some(score),
            ..Default::default()
        }
    }

    #[test]
    fn lineup_is_free_first_and_ranked() {
        let corpus = corpus_with(vec![
            free_model("a/strong", 900.0),
            free_model("b/mid", 500.0),
            free_model("c/weak", 100.0),
        ]);
        let health = HealthStore::default();
        let lineup = build_lineup(&corpus, &health, 2);
        assert_eq!(lineup.len(), 2);
        assert_eq!(lineup[0].model, "a/strong");
        assert!((lineup[0].base_weight - 1.0).abs() < 1e-9);
        assert!(lineup[1].base_weight < 1.0);
    }

    #[test]
    fn paid_models_never_enter_lineup() {
        let mut paid = free_model("d/paid", 999.0);
        paid.free = false;
        paid.route_candidate = false;
        let corpus = corpus_with(vec![paid, free_model("a/free", 100.0)]);
        let lineup = build_lineup(&corpus, &HealthStore::default(), 5);
        assert_eq!(lineup.len(), 1);
        assert_eq!(lineup[0].model, "a/free");
    }
}
