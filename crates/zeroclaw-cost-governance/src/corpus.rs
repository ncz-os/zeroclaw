//! The classified model corpus produced by the corpus builder/refresh job.
//!
//! Each [`ModelEntry`] carries free/paid classification, arena-derived
//! capability weights (overall + coding/SWE), and benched latency/throughput,
//! combined into a single `agentic_score` the router can sort on.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub leaf: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub route_candidate: bool,
    #[serde(default)]
    pub free: bool,
    #[serde(default)]
    pub paid: bool,
    #[serde(default)]
    pub gated_reason: Option<String>,

    #[serde(default)]
    pub arena_overall_elo: Option<f64>,
    #[serde(default)]
    pub arena_coding_elo: Option<f64>,
    #[serde(default)]
    pub arena_webdev_elo: Option<f64>,
    #[serde(default)]
    pub w_overall: Option<f64>,
    #[serde(default)]
    pub w_coding: Option<f64>,
    #[serde(default)]
    pub w_swe: Option<f64>,

    // populated by the latency bench / merge
    #[serde(default)]
    pub ttft_ms_p50: Option<f64>,
    #[serde(default)]
    pub tok_per_s_p50: Option<f64>,
    #[serde(default)]
    pub total_ms_p50: Option<f64>,
    #[serde(default)]
    pub latency_score: Option<f64>,
    #[serde(default)]
    pub latency_class: Option<String>,
    #[serde(default)]
    pub agentic_score: Option<f64>,
}

impl ModelEntry {
    /// SWE capability ELO, preferring the text-coding arena then webdev.
    pub fn swe_elo(&self) -> Option<f64> {
        self.arena_coding_elo.or(self.arena_webdev_elo)
    }
    /// True when this is a free, general chat model we can route real work to.
    pub fn routable(&self) -> bool {
        self.free && self.route_candidate && self.kind == "chat"
    }

    /// Minimal entry for a newly-observed served model id. Deliberately
    /// conservative: it is NOT classified free and NOT route-eligible until the
    /// full corpus builder classifies + benches it, so a refresh can never
    /// silently promote an unknown (possibly paid) model into routing.
    fn from_served_id(id: &str) -> Self {
        let (host, leaf) = match id.split_once('/') {
            Some((h, l)) => (h.to_string(), l.to_string()),
            None => (String::new(), id.to_string()),
        };
        let family = if host.is_empty() {
            id.to_string()
        } else {
            host.clone()
        };
        ModelEntry {
            id: id.to_string(),
            host,
            leaf,
            family,
            kind: "chat".into(),
            route_candidate: false,
            free: false,
            paid: false,
            gated_reason: Some("new: needs classification + bench (run corpus builder)".into()),
            ..Default::default()
        }
    }
}

/// Outcome of reconciling the corpus against the live served-model list.
#[derive(Debug, Default)]
pub struct RefreshReport {
    pub added: Vec<String>,
    pub retired: Vec<String>,
    pub kept: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corpus {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub arena_date: String,
    #[serde(default)]
    pub count: usize,
    pub models: Vec<ModelEntry>,
}

impl Corpus {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::Error::msg(format!("read corpus {}: {e}", path.display())))?;
        // Lenient parse: deserialize the models array element-by-element so one
        // bad entry doesn't take down every command. `id` stays required.
        let root: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            anyhow::Error::msg(format!("corpus {} is not valid JSON: {e}", path.display()))
        })?;

        let str_field = |k: &str| {
            root.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };

        let mut models = Vec::new();
        let mut skipped = 0usize;
        match root.get("models").and_then(|v| v.as_array()) {
            Some(arr) => {
                for item in arr {
                    match serde_json::from_value::<ModelEntry>(item.clone()) {
                        Ok(m) if !m.id.is_empty() => models.push(m),
                        _ => skipped += 1,
                    }
                }
            }
            None => anyhow::bail!("corpus {}: missing `models` array", path.display()),
        }
        if skipped > 0 {
            eprintln!(
                "zoder: warning: corpus skipped {skipped} invalid model entr{} (missing/invalid `id` or fields)",
                if skipped == 1 { "y" } else { "ies" }
            );
        }

        Ok(Corpus {
            source: str_field("source"),
            arena_date: str_field("arena_date"),
            count: models.len(),
            models,
        })
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn free_chat(&self) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(|m| m.routable())
    }

    /// Reconcile the corpus with the live set of served model ids. New ids are
    /// added as unclassified/non-routable; ids no longer served are retired
    /// (kept for history but removed from routing). Existing classification and
    /// scores are preserved so a refresh never loses bench data.
    pub fn reconcile(&mut self, served: &[String]) -> RefreshReport {
        let mut report = RefreshReport::default();
        let served_set: std::collections::HashSet<&str> =
            served.iter().map(|s| s.as_str()).collect();
        let existing: std::collections::HashSet<String> =
            self.models.iter().map(|m| m.id.clone()).collect();

        for id in served {
            if !existing.contains(id) {
                self.models.push(ModelEntry::from_served_id(id));
                report.added.push(id.clone());
            } else {
                report.kept += 1;
            }
        }
        for m in self.models.iter_mut() {
            // Only models still in the routing pool (route_candidate) are
            // retired. Keying off `route_candidate` (the field we clear) keeps
            // this idempotent: once retired, a later refresh won't re-report it.
            if !served_set.contains(m.id.as_str()) && m.route_candidate {
                m.route_candidate = false;
                m.gated_reason = Some("retired: not currently served".into());
                report.retired.push(m.id.clone());
            }
        }
        self.count = self.models.len();
        report
    }

    /// Persist atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
