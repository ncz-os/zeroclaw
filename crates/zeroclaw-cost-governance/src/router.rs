//! Smart router: pick the best FREE model for a task by capability x latency x
//! live health, with a deterministic cross-family fallback chain.

use crate::corpus::{Corpus, ModelEntry};
use crate::health::HealthStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// High-frequency agentic loops: optimize latency/throughput first.
    Fast,
    /// Hard reasoning/codegen: optimize capability (SWE) first.
    Strong,
    /// Balanced default: the composite agentic score.
    Auto,
}

impl Tier {
    pub fn parse(s: &str) -> Tier {
        match s.to_ascii_lowercase().as_str() {
            "fast" => Tier::Fast,
            "strong" | "codex" => Tier::Strong,
            _ => Tier::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Route {
    pub primary: String,
    pub fallbacks: Vec<String>,
    pub reason: String,
}

pub struct Router<'a> {
    corpus: &'a Corpus,
    health: &'a HealthStore,
}

impl<'a> Router<'a> {
    pub fn new(corpus: &'a Corpus, health: &'a HealthStore) -> Self {
        Self { corpus, health }
    }

    fn rank_key(m: &ModelEntry, tier: Tier) -> f64 {
        match tier {
            Tier::Fast => m.latency_score.or(m.agentic_score).unwrap_or(0.0),
            Tier::Strong => m.w_swe.or(m.agentic_score).unwrap_or(0.0),
            Tier::Auto => m.agentic_score.or(m.w_swe).unwrap_or(0.0),
        }
    }

    /// Ordered free candidates for a tier, skipping open circuit breakers.
    /// A half-open breaker (cooldown elapsed) is selectable so models recover.
    fn candidates(&self, tier: Tier) -> Vec<&ModelEntry> {
        // Precompute the rank key once per model instead of 3x per comparison.
        let mut keyed: Vec<(f64, &ModelEntry)> = self
            .corpus
            .free_chat()
            .filter(|m| !self.health.breaker_open(&m.id))
            .map(|m| (Self::rank_key(m, tier), m))
            .filter(|(k, _)| *k > 0.0)
            .collect();
        keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        keyed.into_iter().map(|(_, m)| m).collect()
    }

    /// Pick a primary + a cross-family fallback chain.
    pub fn select(&self, tier: Tier) -> anyhow::Result<Route> {
        let ranked = self.candidates(tier);
        let primary = ranked.first().ok_or_else(|| {
            anyhow::Error::msg(format!("no healthy free model available for tier {tier:?}"))
        })?;

        // Fallback chain: highest-ranked models of a DIFFERENT family than the
        // primary (diversity dodges a family-wide outage), then same-family.
        let mut fallbacks: Vec<String> = Vec::new();
        for m in ranked.iter().skip(1) {
            if m.family != primary.family && fallbacks.len() < 3 {
                fallbacks.push(m.id.clone());
            }
        }
        for m in ranked.iter().skip(1) {
            if fallbacks.len() >= 4 {
                break;
            }
            if m.family == primary.family {
                fallbacks.push(m.id.clone());
            }
        }

        let reason = format!(
            "tier={:?} pick={} (swe={:?} ttft={:?}ms tok/s={:?} agentic={:?}) free=$0",
            tier,
            primary.id,
            primary.swe_elo(),
            primary.ttft_ms_p50,
            primary.tok_per_s_p50,
            primary.agentic_score,
        );
        Ok(Route {
            primary: primary.id.clone(),
            fallbacks,
            reason,
        })
    }
}
