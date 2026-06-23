//! Multi-model consensus review for ZeroClaw.
//!
//! Fan a single prompt out to a panel of models ("muses") served by any
//! [`ModelProvider`], score each by a health-weighted dynamic weight, then pick
//! a verdict by mode:
//!
//! - `single`: one highest-weighted muse.
//! - `panel`: fan out to all; winner = highest-scoring successful muse.
//! - `majority`: fan out, then cluster responses by pairwise text similarity
//!   (Ratcliff/Obershelp) with union-find; quorum is reached when the largest
//!   agreement cluster is `>= ceil(quorum * n)`; the winner is the
//!   highest-scoring muse inside that cluster.
//! - `debate`: two rounds; each muse refines its answer after seeing the
//!   others' round-1 answers; consensus is computed on round 2.
//!
//! The consensus math (`dynamic_weight`, `text_similarity`, `compute_consensus`,
//! `compute_quorum`) is pure and provider-agnostic, so it is unit-testable in
//! isolation. Orchestration depends only on the [`ModelProvider`] trait from
//! `zeroclaw-api` — no provider SDK, no config, no storage. Callers build the
//! [`Panelist`] lineup (e.g. free-first selection from a model corpus) and own
//! cost recording; this crate adds only the consensus capability.

use std::collections::BTreeMap;

use futures_util::future::join_all;
use serde::Serialize;
use zeroclaw_api::model_provider::{ChatMessage, ChatRequest, ModelProvider};

/// Cap normalized text length fed to the similarity metric so a pair of very
/// long reviews cannot blow up the O(n*m) matching. Bounds clustering latency
/// without changing results for normal review sizes.
const SIM_MAX_CHARS: usize = 8000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewMode {
    Single,
    Panel,
    Majority,
    Debate,
}

impl ReviewMode {
    pub fn parse(s: &str) -> ReviewMode {
        match s.to_ascii_lowercase().as_str() {
            "single" => ReviewMode::Single,
            "majority" => ReviewMode::Majority,
            "debate" => ReviewMode::Debate,
            _ => ReviewMode::Panel,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MuseStatus {
    Success,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct MuseResult {
    pub model: String,
    pub status: MuseStatus,
    pub response_text: String,
    pub final_score: f64,
    pub base_weight: f64,
    pub success_rate: f64,
    pub latency_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Consensus {
    /// The winning muse's response (the VERDICT). Empty if none succeeded.
    pub verdict: String,
    pub consensus_score: f64,
    pub winning_muse: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quorum_reached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quorum_threshold: Option<f64>,
    pub similarity_pairs: BTreeMap<String, f64>,
    pub muses: Vec<MuseResult>,
    pub latency_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// A panel member: which model, and the inputs to its dynamic weight. Built by
/// the caller — this crate is agnostic to where `base_weight` (capability) and
/// `success_rate` (health) come from.
#[derive(Clone, Debug)]
pub struct Panelist {
    pub model: String,
    pub base_weight: f64,
    pub success_rate: f64,
}

impl Panelist {
    pub fn new(model: impl Into<String>, base_weight: f64, success_rate: f64) -> Self {
        Self {
            model: model.into(),
            base_weight,
            success_rate,
        }
    }
}

// ── Pure consensus math ──────────────────────────────────────────────────────

/// `base_weight * (0.5 + 0.5 * success_rate)`, rounded to 4 dp.
pub fn dynamic_weight(base_weight: f64, success_rate: f64) -> f64 {
    let w = base_weight * (0.5 + 0.5 * success_rate.clamp(0.0, 1.0));
    (w * 10_000.0).round() / 10_000.0
}

fn normalize(s: &str) -> Vec<char> {
    let joined = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut v: Vec<char> = joined.chars().collect();
    if v.len() > SIM_MAX_CHARS {
        v.truncate(SIM_MAX_CHARS);
    }
    v
}

/// Longest common contiguous block: returns (i_in_a, j_in_b, len).
fn longest_match(a: &[char], b: &[char]) -> (usize, usize, usize) {
    let (n, m) = (a.len(), b.len());
    let mut prev = vec![0usize; m + 1];
    let mut best = (0usize, 0usize, 0usize);
    for i in 1..=n {
        let mut cur = vec![0usize; m + 1];
        for j in 1..=m {
            if a[i - 1] == b[j - 1] {
                cur[j] = prev[j - 1] + 1;
                if cur[j] > best.2 {
                    best = (i - cur[j], j - cur[j], cur[j]);
                }
            }
        }
        prev = cur;
    }
    best
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (i, j, k) = longest_match(a, b);
    if k == 0 {
        return 0;
    }
    matching_chars(&a[..i], &b[..j]) + k + matching_chars(&a[i + k..], &b[j + k..])
}

/// Ratcliff/Obershelp ratio in [0,1], matching Python's
/// `SequenceMatcher(None, a, b).ratio()` on whitespace-normalized lowercase.
pub fn text_similarity(a: &str, b: &str) -> f64 {
    let a = normalize(a);
    let b = normalize(b);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let m = matching_chars(&a, &b);
    2.0 * m as f64 / (a.len() + b.len()) as f64
}

/// Winner = successful muse with the highest `final_score`.
/// Returns (verdict_text, consensus_score, winning_muse).
pub fn compute_consensus(muses: &[MuseResult]) -> (String, f64, Option<String>) {
    let winner = muses
        .iter()
        .filter(|m| m.status == MuseStatus::Success)
        .max_by(|a, b| {
            a.final_score
                .partial_cmp(&b.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    match winner {
        Some(w) => (
            w.response_text.clone(),
            w.final_score,
            Some(w.model.clone()),
        ),
        None => (String::new(), 0.0, None),
    }
}

pub struct QuorumResult {
    pub quorum_reached: bool,
    pub quorum_muses: Vec<String>,
    pub consensus_score: f64,
    pub similarity_pairs: BTreeMap<String, f64>,
}

/// Cluster successful responses by pairwise similarity >= `quorum` with
/// union-find; quorum is reached when the largest cluster is >= ceil(quorum*n).
pub fn compute_quorum(muses: &[MuseResult], quorum: f64) -> QuorumResult {
    let successes: Vec<(&str, &str)> = muses
        .iter()
        .filter(|m| m.status == MuseStatus::Success && !m.response_text.trim().is_empty())
        .map(|m| (m.model.as_str(), m.response_text.as_str()))
        .collect();

    if successes.len() < 2 {
        return QuorumResult {
            quorum_reached: false,
            quorum_muses: Vec::new(),
            consensus_score: 0.0,
            similarity_pairs: BTreeMap::new(),
        };
    }

    let names: Vec<&str> = successes.iter().map(|(n, _)| *n).collect();
    let mut parent: Vec<usize> = (0..names.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }

    let mut similarity_pairs: BTreeMap<String, f64> = BTreeMap::new();
    let mut best_score = 0.0f64;
    for i in 0..successes.len() {
        for j in (i + 1)..successes.len() {
            let score = text_similarity(successes[i].1, successes[j].1);
            let rounded = (score * 10_000.0).round() / 10_000.0;
            similarity_pairs.insert(format!("{}:{}", successes[i].0, successes[j].0), rounded);
            best_score = best_score.max(score);
            if score >= quorum {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[rj] = ri;
                }
            }
        }
    }

    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..names.len() {
        let root = find(&mut parent, i);
        components.entry(root).or_default().push(i);
    }
    let quorum_size = (2usize).max((quorum * names.len() as f64).ceil() as usize);
    let largest = components
        .values()
        .max_by_key(|v| v.len())
        .cloned()
        .unwrap_or_default();
    let quorum_reached = largest.len() >= quorum_size;

    let consensus_score = if quorum_reached && largest.len() > 1 {
        let mut max_in = 0.0f64;
        for a in 0..largest.len() {
            for b in (a + 1)..largest.len() {
                let (na, nb) = (names[largest[a]], names[largest[b]]);
                let s = similarity_pairs
                    .get(&format!("{na}:{nb}"))
                    .or_else(|| similarity_pairs.get(&format!("{nb}:{na}")))
                    .copied()
                    .unwrap_or(0.0);
                max_in = max_in.max(s);
            }
        }
        max_in
    } else {
        best_score.min((quorum - 0.01).max(0.0))
    };

    let quorum_muses = if quorum_reached {
        largest.iter().map(|&i| names[i].to_string()).collect()
    } else {
        Vec::new()
    };

    QuorumResult {
        quorum_reached,
        quorum_muses,
        consensus_score: (consensus_score * 10_000.0).round() / 10_000.0,
        similarity_pairs,
    }
}

// ── Async orchestration over ModelProvider ───────────────────────────────────

/// Fan out one prompt per panelist concurrently and assemble scored results.
async fn fanout<P: ModelProvider + ?Sized>(
    provider: &P,
    jobs: &[(Panelist, Vec<ChatMessage>)],
    temp: Option<f64>,
) -> Vec<MuseResult> {
    let calls = jobs.iter().map(|(panelist, history)| async move {
        let started = std::time::Instant::now();
        let req = ChatRequest {
            messages: history,
            tools: None,
            thinking: None,
        };
        let res = provider.chat(req, &panelist.model, temp).await;
        (panelist, started.elapsed().as_millis() as u64, res)
    });

    let mut out: Vec<MuseResult> = Vec::with_capacity(jobs.len());
    for (p, latency_ms, res) in join_all(calls).await {
        match res {
            Ok(resp) => {
                let (tokens_in, tokens_out) = resp
                    .usage
                    .as_ref()
                    .map(|u| (u.input_tokens.unwrap_or(0), u.output_tokens.unwrap_or(0)))
                    .unwrap_or((0, 0));
                out.push(MuseResult {
                    model: p.model.clone(),
                    status: MuseStatus::Success,
                    response_text: resp.text.unwrap_or_default(),
                    final_score: dynamic_weight(p.base_weight, p.success_rate),
                    base_weight: p.base_weight,
                    success_rate: p.success_rate,
                    latency_ms,
                    tokens_in,
                    tokens_out,
                    error: None,
                });
            }
            Err(e) => out.push(MuseResult {
                model: p.model.clone(),
                status: MuseStatus::Error,
                response_text: String::new(),
                final_score: 0.0,
                base_weight: p.base_weight,
                success_rate: p.success_rate,
                latency_ms,
                tokens_in: 0,
                tokens_out: 0,
                error: Some(e.to_string()),
            }),
        }
    }
    out.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn assemble(muses: Vec<MuseResult>, quorum: Option<f64>) -> Consensus {
    let latency_ms = muses.iter().map(|m| m.latency_ms).max().unwrap_or(0);
    let tokens_in = muses.iter().map(|m| m.tokens_in).sum();
    let tokens_out = muses.iter().map(|m| m.tokens_out).sum();
    let (mut verdict, mut score, mut winner) = compute_consensus(&muses);
    let mut similarity_pairs = BTreeMap::new();
    let (mut q_reached, mut q_thresh) = (None, None);

    if let Some(q) = quorum {
        let qr = compute_quorum(&muses, q);
        similarity_pairs = qr.similarity_pairs;
        q_reached = Some(qr.quorum_reached);
        q_thresh = Some(q);
        score = qr.consensus_score;
        if qr.quorum_reached && !qr.quorum_muses.is_empty() {
            // Winner = highest-scoring muse inside the agreement cluster.
            if let Some(w) = muses
                .iter()
                .filter(|m| qr.quorum_muses.contains(&m.model))
                .max_by(|a, b| {
                    a.final_score
                        .partial_cmp(&b.final_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            {
                winner = Some(w.model.clone());
                verdict = w.response_text.clone();
            }
        }
    }

    Consensus {
        verdict,
        consensus_score: score,
        winning_muse: winner,
        quorum_reached: q_reached,
        quorum_threshold: q_thresh,
        similarity_pairs,
        muses,
        latency_ms,
        tokens_in,
        tokens_out,
    }
}

/// Run a review with the given mode. `prompt` is the fully-framed review prompt;
/// every panelist's model must be servable by `provider`.
pub async fn run_review<P: ModelProvider + ?Sized>(
    provider: &P,
    lineup: Vec<Panelist>,
    prompt: &str,
    mode: ReviewMode,
    temp: Option<f64>,
    quorum: f64,
) -> anyhow::Result<Consensus> {
    if lineup.is_empty() {
        anyhow::bail!("no models available for review (empty lineup)");
    }

    match mode {
        ReviewMode::Single => {
            let one = vec![(
                lineup.into_iter().next().expect("lineup non-empty"),
                vec![ChatMessage::user(prompt)],
            )];
            Ok(assemble(fanout(provider, &one, temp).await, None))
        }
        ReviewMode::Panel => {
            let jobs: Vec<(Panelist, Vec<ChatMessage>)> = lineup
                .into_iter()
                .map(|p| (p, vec![ChatMessage::user(prompt)]))
                .collect();
            Ok(assemble(fanout(provider, &jobs, temp).await, None))
        }
        ReviewMode::Majority => {
            let jobs: Vec<(Panelist, Vec<ChatMessage>)> = lineup
                .into_iter()
                .map(|p| (p, vec![ChatMessage::user(prompt)]))
                .collect();
            Ok(assemble(fanout(provider, &jobs, temp).await, Some(quorum)))
        }
        ReviewMode::Debate => {
            // Round 1: everyone answers the original prompt.
            let r1_jobs: Vec<(Panelist, Vec<ChatMessage>)> = lineup
                .iter()
                .cloned()
                .map(|p| (p, vec![ChatMessage::user(prompt)]))
                .collect();
            let round1 = fanout(provider, &r1_jobs, temp).await;

            // Round 2: each muse refines after seeing the others' round-1 text.
            let r2_jobs: Vec<(Panelist, Vec<ChatMessage>)> = lineup
                .into_iter()
                .map(|p| {
                    let refine = debate_refinement_prompt(prompt, &p.model, &round1);
                    (p, vec![ChatMessage::user(refine)])
                })
                .collect();
            let round2 = fanout(provider, &r2_jobs, temp).await;
            Ok(assemble(round2, Some(quorum)))
        }
    }
}

/// Build the round-2 refinement prompt for `current`, embedding the other
/// muses' round-1 reviews.
pub fn debate_refinement_prompt(prompt: &str, current: &str, round1: &[MuseResult]) -> String {
    let mut others = Vec::new();
    for m in round1 {
        if m.model == current {
            continue;
        }
        let t = m.response_text.trim();
        if !t.is_empty() {
            others.push(format!("{}: {}", m.model, t));
        }
    }
    let context = if others.is_empty() {
        "No other round-1 responses were available.".to_string()
    } else {
        others.join("\n\n")
    };
    format!(
        "Original review prompt:\n{prompt}\n\nRound 1 reviews from the other muses:\n{context}\n\n\
         Refine your review. Address useful objections, keep what still holds, and be explicit \
         where you disagree."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn muse(model: &str, text: &str, score: f64) -> MuseResult {
        MuseResult {
            model: model.into(),
            status: MuseStatus::Success,
            response_text: text.into(),
            final_score: score,
            base_weight: 0.8,
            success_rate: 1.0,
            latency_ms: 10,
            tokens_in: 1,
            tokens_out: 1,
            error: None,
        }
    }

    #[test]
    fn dynamic_weight_health_scaling() {
        // 100% success keeps full weight; 0% halves it.
        assert!((dynamic_weight(0.8, 1.0) - 0.8).abs() < 1e-9);
        assert!((dynamic_weight(0.8, 0.0) - 0.4).abs() < 1e-9);
        assert!((dynamic_weight(0.8, 0.5) - 0.6).abs() < 1e-9);
    }

    #[test]
    fn similarity_bounds() {
        assert!((text_similarity("same text here", "same text here") - 1.0).abs() < 1e-9);
        assert_eq!(text_similarity("", "anything"), 0.0);
        let s = text_similarity("the cat sat on the mat", "the dog sat on the rug");
        assert!(s > 0.4 && s < 1.0, "got {s}");
    }

    #[test]
    fn consensus_picks_highest_score() {
        let muses = vec![muse("a", "ans a", 0.7), muse("b", "ans b", 0.9)];
        let (verdict, score, winner) = compute_consensus(&muses);
        assert_eq!(winner.as_deref(), Some("b"));
        assert_eq!(verdict, "ans b");
        assert!((score - 0.9).abs() < 1e-9);
    }

    #[test]
    fn quorum_clusters_agreeing_muses() {
        let common = "The change is correct and should be approved. Looks good to merge.";
        let muses = vec![
            muse("a", common, 0.9),
            muse("b", common, 0.8),
            muse("c", common, 0.7),
            muse(
                "d",
                "Reject: this is totally broken and unrelated nonsense xyz.",
                0.95,
            ),
        ];
        let qr = compute_quorum(&muses, 0.66);
        assert!(qr.quorum_reached);
        assert_eq!(qr.quorum_muses.len(), 3);
        assert!(qr.quorum_muses.contains(&"a".to_string()));
        assert!(!qr.quorum_muses.contains(&"d".to_string()));
    }

    #[test]
    fn quorum_fails_when_all_disagree() {
        let muses = vec![
            muse("a", "alpha unique answer one", 0.9),
            muse("b", "completely different beta two", 0.8),
            muse("c", "yet another gamma three entirely", 0.7),
        ];
        let qr = compute_quorum(&muses, 0.66);
        assert!(!qr.quorum_reached);
        assert!(qr.quorum_muses.is_empty());
    }
}
