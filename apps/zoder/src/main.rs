//! zoder — a cost-governed, free-first coding CLI distributed on top of
//! ZeroClaw.
//!
//! zoder is purely additive: it adds the cost-governance and consensus-review
//! commands (`review`, `route`, `spend`, `models`, `corpus`) and delegates
//! everything else — `agent`, `auth`, `acp`, `gateway`, `doctor`, ... — to the
//! `zeroclaw` binary via an exec passthrough. That gives the full Codex-class
//! command surface without reimplementing any of it.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use zeroclaw_api::model_provider::{ChatMessage, ChatRequest};
use zeroclaw_config::cost::{CostTracker, TokenUsage};
use zeroclaw_config::schema::CostConfig;
use zeroclaw_consensus::{Panelist, ReviewMode, run_review};
use zeroclaw_cost_governance::{
    Corpus, HealthStore, RankedModel, Router, Tier, build_lineup, lineup_from_models,
};
use zeroclaw_providers::ModelProvider;
use zeroclaw_providers::compatible::{AuthStyle, OpenAiCompatibleModelProvider};

#[derive(Parser, Debug)]
#[command(
    name = "zoder",
    about = "Cost-governed, free-first coding CLI — a ZeroClaw distro.",
    long_about = "zoder adds free-first routing, multi-model consensus review, and spend \
visibility on top of ZeroClaw. Unrecognized subcommands (agent, auth, acp, gateway, \
doctor, ...) pass through to the `zeroclaw` binary.",
    disable_help_subcommand = true,
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Fan a code review out to a free model panel and reach consensus.
    Review(ReviewArgs),
    /// Review, then have a zeroclaw agent apply the fixes in place, looping
    /// until the panel approves (or --rounds is hit).
    Fix(FixArgs),
    /// Show the free-first model the router would pick for a tier.
    Route(RouteArgs),
    /// Summarize spend from ZeroClaw's cost ledger (unified with `agent`).
    Spend(SpendArgs),
    /// List the live model ids served by the configured endpoint.
    Models,
    /// Manage the classified model corpus.
    Corpus {
        #[command(subcommand)]
        cmd: CorpusCmd,
    },
    /// Any other subcommand is passed straight through to `zeroclaw`.
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(clap::Args, Debug)]
struct ReviewArgs {
    /// Review mode: single | panel | majority | debate.
    #[arg(long, default_value = "panel")]
    mode: String,
    /// Explicit, comma-separated model ids (bypasses corpus selection).
    #[arg(long)]
    models: Option<String>,
    /// Max panel size when selecting from the free corpus.
    #[arg(long, default_value_t = 3)]
    panel: usize,
    /// Agreement threshold for majority/debate quorum (0..1).
    #[arg(long, default_value_t = 0.66)]
    quorum: f64,
    /// Sampling temperature.
    #[arg(long)]
    temperature: Option<f64>,
    /// Extra instruction prepended to the framed review prompt.
    #[arg(long)]
    instruction: Option<String>,
    /// Review this text instead of the git diff.
    #[arg(long)]
    text: Option<String>,
    /// Emit the full consensus as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug)]
struct FixArgs {
    /// zeroclaw agent alias that applies the edits (or set $ZODER_FIX_AGENT).
    /// The alias must be allowed to edit files non-interactively.
    #[arg(long)]
    agent: Option<String>,
    /// Max review->fix rounds.
    #[arg(long, default_value_t = 3)]
    rounds: usize,
    /// Review mode: single | panel | majority | debate.
    #[arg(long, default_value = "majority")]
    mode: String,
    /// Explicit, comma-separated model ids for the review panel.
    #[arg(long)]
    models: Option<String>,
    /// Max panel size when selecting from the free corpus.
    #[arg(long, default_value_t = 3)]
    panel: usize,
    /// Agreement threshold for majority/debate quorum (0..1).
    #[arg(long, default_value_t = 0.66)]
    quorum: f64,
    /// Sampling temperature for the review.
    #[arg(long)]
    temperature: Option<f64>,
    /// Extra instruction prepended to the framed review prompt.
    #[arg(long)]
    instruction: Option<String>,
    /// Review each round but never invoke the agent (show what it would do).
    #[arg(long)]
    dry_run: bool,
}

#[derive(clap::Args, Debug)]
struct RouteArgs {
    /// Routing tier: fast | strong | auto.
    #[arg(long, default_value = "auto")]
    tier: String,
}

#[derive(clap::Args, Debug)]
struct SpendArgs {
    /// Window: today | week | month | year | all.
    #[arg(long, default_value = "today")]
    period: String,
}

#[derive(Subcommand, Debug)]
enum CorpusCmd {
    /// Reconcile the corpus against the live served-model list.
    Refresh,
    /// Show a summary of the current corpus.
    Show,
    /// Latency-bench models and write throughput scores into the corpus so
    /// `zoder route` can rank by measured speed.
    Bench(BenchArgs),
}

#[derive(clap::Args, Debug)]
struct BenchArgs {
    /// Comma-separated model ids to bench (defaults to the free corpus).
    #[arg(long)]
    models: Option<String>,
    /// Bench every served model, not just the free corpus.
    #[arg(long)]
    all: bool,
    /// Samples per model; the median is recorded.
    #[arg(long, default_value_t = 3)]
    samples: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Passthrough is synchronous (it replaces this process); everything else
    // needs the async provider, so only spin up tokio when we keep control.
    if let Command::External(args) = &cli.command {
        return passthrough(args);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?
        .block_on(run(cli.command))
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::Review(args) => cmd_review(args).await,
        Command::Fix(args) => cmd_fix(args).await,
        Command::Route(args) => cmd_route(args),
        Command::Spend(args) => cmd_spend(args),
        Command::Models => cmd_models().await,
        Command::Corpus { cmd } => cmd_corpus(cmd).await,
        Command::External(_) => unreachable!("handled before runtime"),
    }
}

// ── paths & resources ────────────────────────────────────────────────────────

/// ZeroClaw's config/data dir. Reused so zoder's spend reads the same ledger
/// `zeroclaw agent` writes.
fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ZEROCLAW_CONFIG_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    directories::UserDirs::new()
        .map(|u| u.home_dir().join(".zeroclaw"))
        .unwrap_or_else(|| PathBuf::from(".zeroclaw"))
}

fn zoder_dir() -> PathBuf {
    config_dir().join("zoder")
}
fn corpus_path() -> PathBuf {
    zoder_dir().join("corpus.json")
}
fn health_path() -> PathBuf {
    zoder_dir().join("health.json")
}

/// Build the inference engine from env. zoder reads the endpoint from
/// `ZODER_BASE_URL` / `ZODER_API_KEY` for now; config.toml resolution is a
/// follow-up that will source these from ZeroClaw's provider config.
fn provider() -> Result<OpenAiCompatibleModelProvider> {
    let base = std::env::var("ZODER_BASE_URL").map_err(|_| {
        anyhow::Error::msg(
            "set ZODER_BASE_URL to your OpenAI-compatible endpoint (e.g. https://host/v1)",
        )
    })?;
    let key = std::env::var("ZODER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    Ok(OpenAiCompatibleModelProvider::new(
        "zoder",
        "zoder",
        &base,
        key.as_deref(),
        AuthStyle::Bearer,
    )
    .with_public_model_listing())
}

fn tracker() -> Result<CostTracker> {
    let cfg = CostConfig {
        enabled: true,
        ..Default::default()
    };
    CostTracker::new(cfg, &config_dir()).context("open cost ledger")
}

fn load_corpus() -> Corpus {
    Corpus::load(&corpus_path()).unwrap_or_else(|_| Corpus {
        source: "empty".into(),
        arena_date: String::new(),
        count: 0,
        models: Vec::new(),
    })
}

fn to_panel(ranked: Vec<RankedModel>) -> Vec<Panelist> {
    ranked
        .into_iter()
        .map(|r| Panelist::new(r.model, r.base_weight, r.success_rate))
        .collect()
}

/// Panel precedence: explicit `--models`, then the free corpus lineup, then a
/// default panel from `$ZODER_MODELS` so daily use needs no repeated flags.
fn resolve_lineup(
    corpus: &Corpus,
    health: &HealthStore,
    models: Option<&str>,
    panel: usize,
) -> Vec<Panelist> {
    if let Some(csv) = models {
        return to_panel(lineup_from_models(&parse_csv(csv), corpus, health));
    }
    let from_corpus = build_lineup(corpus, health, panel);
    if !from_corpus.is_empty() {
        return to_panel(from_corpus);
    }
    match std::env::var("ZODER_MODELS").ok().filter(|s| !s.is_empty()) {
        Some(csv) => to_panel(lineup_from_models(&parse_csv(&csv), corpus, health)),
        None => Vec::new(),
    }
}

/// Record per-muse spend (free models at $0) into the unified ledger and fold
/// each call's outcome into the routing health signal (then persist it).
fn record_consensus(consensus: &zeroclaw_consensus::Consensus, health: &mut HealthStore) {
    if let Ok(tr) = tracker() {
        for m in &consensus.muses {
            let _ = tr.record_usage_with_agent(
                TokenUsage::new(&m.model, m.tokens_in, m.tokens_out, 0, 0.0, 0.0, 0.0),
                Some("zoder-review"),
            );
        }
    }
    for m in &consensus.muses {
        match m.status {
            zeroclaw_consensus::MuseStatus::Success => {
                health.record_success(&m.model, m.latency_ms as f64)
            }
            zeroclaw_consensus::MuseStatus::Error => {
                health.record_failure(&m.model, m.error.as_deref().unwrap_or("error"))
            }
        }
    }
    let _ = health.save();
}

// ── review ───────────────────────────────────────────────────────────────────

async fn cmd_review(args: ReviewArgs) -> Result<()> {
    let subject = match &args.text {
        Some(t) => t.clone(),
        None => read_git_diff()?,
    };
    if subject.trim().is_empty() {
        anyhow::bail!("nothing to review: no --text and no staged/unstaged git diff found");
    }
    let prompt = frame_review(&subject, args.instruction.as_deref());

    let corpus = load_corpus();
    let mut health = HealthStore::load(&health_path());
    let lineup = resolve_lineup(&corpus, &health, args.models.as_deref(), args.panel);
    if lineup.is_empty() {
        anyhow::bail!(
            "no models for the panel. Pass --models a,b,c, set $ZODER_MODELS, or build a \
             classified corpus (zoder corpus refresh && zoder corpus bench)."
        );
    }

    let provider = provider()?;
    let mode = ReviewMode::parse(&args.mode);
    let consensus = run_review(
        &provider,
        lineup,
        &prompt,
        mode,
        args.temperature,
        args.quorum,
    )
    .await?;

    record_consensus(&consensus, &mut health);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&consensus)?);
        return Ok(());
    }
    print_review(&consensus);
    Ok(())
}

// ── fix ───────────────────────────────────────────────────────────────────────

async fn cmd_fix(args: FixArgs) -> Result<()> {
    let agent = args.agent.clone().or_else(|| {
        std::env::var("ZODER_FIX_AGENT")
            .ok()
            .filter(|s| !s.is_empty())
    });
    if !args.dry_run && agent.is_none() {
        anyhow::bail!(
            "no fix agent: pass --agent <alias> or set $ZODER_FIX_AGENT (the alias must be \
             allowed to edit files non-interactively). Use --dry-run to preview reviews only."
        );
    }

    let provider = provider()?;
    let corpus = load_corpus();
    let mode = ReviewMode::parse(&args.mode);

    for round in 1..=args.rounds {
        println!("\n=== fix round {round}/{} ===", args.rounds);
        let consensus = match run_fix_review(&provider, &corpus, &args, mode).await? {
            Some(c) => c,
            None if round == 1 => anyhow::bail!(
                "nothing to fix: no staged/unstaged git diff found. Make or stage changes first."
            ),
            // Empty diff after a fix: the working tree matches HEAD again.
            None => {
                println!(
                    "\nworking tree clean after fixes; converged in {} round(s).",
                    round - 1
                );
                return Ok(());
            }
        };
        print_review(&consensus);

        if verdict_approved(&consensus) {
            println!("\napproved by the panel in round {round}; converged.");
            return Ok(());
        }
        if args.dry_run {
            println!(
                "\n[dry-run] would invoke agent '{}' to apply the verdict above.",
                agent.as_deref().unwrap_or("<none>")
            );
            continue;
        }

        let alias = agent.as_deref().expect("checked above");
        println!("\napplying fixes via zeroclaw agent '{alias}' (round {round}) ...");
        run_agent_fix(alias, &consensus.verdict)?;
    }

    if args.dry_run {
        println!(
            "\n[dry-run] previewed {} round(s); no edits applied.",
            args.rounds
        );
        return Ok(());
    }

    // Validate whatever the last round applied.
    println!("\n=== final review ===");
    match run_fix_review(&provider, &corpus, &args, mode).await? {
        None => {
            println!(
                "\nworking tree clean after fixes; converged after {} round(s).",
                args.rounds
            );
            Ok(())
        }
        Some(consensus) => {
            print_review(&consensus);
            if verdict_approved(&consensus) {
                println!("\nconverged after {} round(s).", args.rounds);
                Ok(())
            } else {
                anyhow::bail!(
                    "still not approved after {} round(s); re-run with more --rounds or fix \
                     manually.",
                    args.rounds
                );
            }
        }
    }
}

/// One review pass against the current working-tree diff: frame, resolve the
/// panel, run consensus, and fold the result into spend + health.
async fn run_fix_review(
    provider: &OpenAiCompatibleModelProvider,
    corpus: &Corpus,
    args: &FixArgs,
    mode: ReviewMode,
) -> Result<Option<zeroclaw_consensus::Consensus>> {
    let subject = read_git_diff()?;
    if subject.trim().is_empty() {
        return Ok(None);
    }
    let prompt = frame_review(&subject, args.instruction.as_deref());

    let mut health = HealthStore::load(&health_path());
    let lineup = resolve_lineup(corpus, &health, args.models.as_deref(), args.panel);
    if lineup.is_empty() {
        anyhow::bail!(
            "no models for the panel. Pass --models a,b,c, set $ZODER_MODELS, or build a \
             classified corpus (zoder corpus refresh && zoder corpus bench)."
        );
    }
    let consensus = run_review(
        provider,
        lineup,
        &prompt,
        mode,
        args.temperature,
        args.quorum,
    )
    .await?;
    record_consensus(&consensus, &mut health);
    Ok(Some(consensus))
}

/// Heuristic approval gate: a "request changes" signal vetoes; otherwise an
/// explicit approval signal (plus quorum, when measured) means converged.
fn verdict_approved(c: &zeroclaw_consensus::Consensus) -> bool {
    if c.winning_muse.is_none() {
        return false;
    }
    let v = c.verdict.to_lowercase();
    let requests_changes = v.contains("request change")
        || v.contains("requests change")
        || v.contains("request_changes")
        || v.contains("needs changes")
        || v.contains("changes required")
        || v.contains("changes requested");
    if requests_changes {
        return false;
    }
    let approves = v.contains("approve")
        || v.contains("lgtm")
        || v.contains("looks good")
        || v.contains("no issues");
    // If a quorum was computed, require it; otherwise trust the verdict signal.
    let quorum_ok = c.quorum_reached.unwrap_or(true);
    approves && quorum_ok
}

/// Hand the verdict to a zeroclaw agent (single-shot) to edit files in place.
/// The agent's file tools may be workspace-scoped, so we pass the absolute repo
/// root and the diff to anchor it on the right files.
fn run_agent_fix(alias: &str, verdict: &str) -> Result<()> {
    let root = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let diff = read_git_diff().unwrap_or_default();
    let message = format!(
        "A multi-model code review requested changes on the git working tree rooted at \
         {root}. Edit the affected files in place under that absolute path (use absolute \
         paths with your file tools). Apply the minimal edits to address every point in \
         the verdict; do not revert unrelated work and keep changes focused. When done, stop.\n\n\
         --- REVIEW VERDICT ---\n{verdict}\n--- END VERDICT ---\n\n\
         --- CURRENT DIFF ---\n{diff}\n--- END DIFF ---"
    );
    let status = std::process::Command::new(zeroclaw_bin())
        .args(["agent", "-a", alias, "-m", &message])
        .status()
        .context("spawn zeroclaw agent")?;
    if !status.success() {
        anyhow::bail!("zeroclaw agent '{alias}' exited with {status}");
    }
    Ok(())
}

fn parse_csv(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn frame_review(subject: &str, instruction: Option<&str>) -> String {
    let extra = instruction.unwrap_or(
        "Review the following change for correctness, bugs, security, and clarity. \
         Give a clear verdict (approve / request changes) and a short, specific rationale.",
    );
    format!("{extra}\n\n--- BEGIN CHANGE ---\n{subject}\n--- END CHANGE ---")
}

fn print_review(c: &zeroclaw_consensus::Consensus) {
    println!("== zoder review ==");
    if let Some(w) = &c.winning_muse {
        println!("verdict by: {w}  (score {:.4})", c.consensus_score);
    } else {
        println!("no successful muse");
    }
    if let Some(q) = c.quorum_reached {
        println!(
            "quorum: {} (threshold {:.2})",
            if q { "reached" } else { "NOT reached" },
            c.quorum_threshold.unwrap_or(0.0)
        );
    }
    println!(
        "panel: {} muses, {} tok in / {} tok out, {} ms",
        c.muses.len(),
        c.tokens_in,
        c.tokens_out,
        c.latency_ms
    );
    for m in &c.muses {
        let status = match m.status {
            zeroclaw_consensus::MuseStatus::Success => "ok",
            zeroclaw_consensus::MuseStatus::Error => "ERR",
        };
        println!("  [{status}] {} (score {:.4})", m.model, m.final_score);
    }
    println!("\n--- VERDICT ---\n{}", c.verdict);
}

fn read_git_diff() -> Result<String> {
    let staged = run_git(&["diff", "--cached"])?;
    if !staged.trim().is_empty() {
        return Ok(staged);
    }
    run_git(&["diff"])
}

fn run_git(args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .context("run git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ── route ────────────────────────────────────────────────────────────────────

fn cmd_route(args: RouteArgs) -> Result<()> {
    let corpus = load_corpus();
    let health = HealthStore::load(&health_path());
    let router = Router::new(&corpus, &health);
    let route = router.select(Tier::parse(&args.tier))?;
    println!("primary:   {}", route.primary);
    if !route.fallbacks.is_empty() {
        println!("fallbacks: {}", route.fallbacks.join(", "));
    }
    println!("reason:    {}", route.reason);
    Ok(())
}

// ── spend ────────────────────────────────────────────────────────────────────

fn cmd_spend(args: SpendArgs) -> Result<()> {
    use chrono::{Datelike, Duration, Utc};
    let tr = tracker()?;
    let now = Utc::now();
    let summary = match args.period.to_ascii_lowercase().as_str() {
        "all" => tr.get_summary_in_bounds(None, None)?,
        "week" => {
            let monday =
                now.date_naive() - Duration::days(now.weekday().num_days_from_monday() as i64);
            let from = monday.and_hms_opt(0, 0, 0).unwrap().and_utc();
            tr.get_summary_in_bounds(Some(from), None)?
        }
        "month" => {
            let first = now.date_naive().with_day(1).unwrap();
            let from = first.and_hms_opt(0, 0, 0).unwrap().and_utc();
            tr.get_summary_in_bounds(Some(from), None)?
        }
        "year" => {
            let first = now.date_naive().with_ordinal(1).unwrap();
            let from = first.and_hms_opt(0, 0, 0).unwrap().and_utc();
            tr.get_summary_in_bounds(Some(from), None)?
        }
        _ => {
            // "today": read from the persisted ledger (a fresh CLI process has
            // no in-memory session totals), so use bounds, not get_summary().
            let from = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
            tr.get_summary_in_bounds(Some(from), None)?
        }
    };

    println!("== zoder spend ({}) ==", args.period);
    println!("cost:     ${:.4}", summary.session_cost_usd);
    println!("day:      ${:.4}", summary.daily_cost_usd);
    println!("month:    ${:.4}", summary.monthly_cost_usd);
    println!("tokens:   {}", summary.total_tokens);
    println!("requests: {}", summary.request_count);
    if !summary.by_model.is_empty() {
        println!("by model:");
        let mut models: Vec<_> = summary.by_model.values().collect();
        models.sort_by(|a, b| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for m in models {
            println!(
                "  {:<40} ${:.4}  ({} req, {} tok)",
                m.model, m.cost_usd, m.request_count, m.total_tokens
            );
        }
    }
    Ok(())
}

// ── models / corpus ──────────────────────────────────────────────────────────

async fn cmd_models() -> Result<()> {
    let provider = provider()?;
    let mut models = provider.list_models().await.context("list models")?;
    models.sort();
    for m in &models {
        println!("{m}");
    }
    println!("\n{} models", models.len());
    Ok(())
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn latency_class(tps: f64) -> &'static str {
    if tps >= 50.0 {
        "fast"
    } else if tps >= 20.0 {
        "medium"
    } else {
        "slow"
    }
}

async fn cmd_corpus_bench(args: BenchArgs) -> Result<()> {
    let provider = provider()?;
    let mut corpus = load_corpus();

    let targets: Vec<String> = if let Some(csv) = &args.models {
        parse_csv(csv)
    } else if args.all {
        provider.list_models().await.context("list models")?
    } else {
        let free: Vec<String> = corpus.free_chat().map(|m| m.id.clone()).collect();
        if free.is_empty() {
            anyhow::bail!(
                "no free models in corpus to bench. Run `zoder corpus refresh` first, or pass \
                 --models a,b,c / --all."
            );
        }
        free
    };

    let samples = args.samples.max(1);
    println!(
        "benching {} model(s), {} sample(s) each...",
        targets.len(),
        samples
    );

    let mut benched = 0usize;
    for id in &targets {
        let mut latencies: Vec<f64> = Vec::new();
        let mut tps: Vec<f64> = Vec::new();
        let mut last_err: Option<String> = None;
        for _ in 0..samples {
            let msgs = [ChatMessage::user("Reply with exactly one word: pong")];
            let req = ChatRequest {
                messages: &msgs,
                tools: None,
                thinking: None,
            };
            let started = std::time::Instant::now();
            match provider.chat(req, id, Some(0.0)).await {
                Ok(resp) => {
                    let ms = started.elapsed().as_millis() as f64;
                    let out = resp
                        .usage
                        .as_ref()
                        .and_then(|u| u.output_tokens)
                        .unwrap_or_else(|| {
                            (resp.text.as_deref().unwrap_or("").len() as u64 / 4).max(1)
                        });
                    let secs = (ms / 1000.0).max(0.001);
                    latencies.push(ms);
                    tps.push(out as f64 / secs);
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        if latencies.is_empty() {
            println!("  [ERR] {id}: {}", last_err.unwrap_or_default());
            continue;
        }
        let ms_p50 = median(&mut latencies);
        let tps_p50 = median(&mut tps);
        if let Some(entry) = corpus.models.iter_mut().find(|m| m.id == *id) {
            entry.total_ms_p50 = Some(ms_p50);
            entry.tok_per_s_p50 = Some(tps_p50);
            entry.latency_score = Some(tps_p50);
            entry.latency_class = Some(latency_class(tps_p50).to_string());
        }
        benched += 1;
        println!(
            "  [ok] {id}: {ms_p50:.0} ms, {tps_p50:.1} tok/s ({})",
            latency_class(tps_p50)
        );
    }

    corpus.save(&corpus_path()).context("save corpus")?;
    println!(
        "benched {benched}/{} model(s); latency scores written. `zoder route --tier fast` now \
         ranks by measured throughput.",
        targets.len()
    );
    Ok(())
}

/// True when an endpoint reports an explicit zero rate for both prompt and
/// completion tokens (e.g. OpenRouter `:free` models). Absent pricing is
/// treated as unknown (not free), so EIH-style paid fleets are never
/// misclassified.
fn is_zero_pricing(p: &zeroclaw_api::model_provider::ModelPricing) -> bool {
    let zero = |v: &Option<String>| {
        v.as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|n| n == 0.0)
            .unwrap_or(false)
    };
    zero(&p.prompt) && zero(&p.completion)
}

async fn cmd_corpus(cmd: CorpusCmd) -> Result<()> {
    match cmd {
        CorpusCmd::Show => {
            let corpus = load_corpus();
            println!("source:    {}", corpus.source);
            println!("models:    {}", corpus.count);
            let free = corpus.free_chat().count();
            println!("free chat: {free}");
            Ok(())
        }
        CorpusCmd::Bench(args) => cmd_corpus_bench(args).await,
        CorpusCmd::Refresh => {
            let provider = provider()?;
            let infos = provider
                .list_models_with_pricing()
                .await
                .context("list models")?;
            let served: Vec<String> = infos.iter().map(|i| i.id.clone()).collect();
            let mut corpus = load_corpus();
            let report = corpus.reconcile(&served);

            // Honest free/paid classification from the endpoint's own pricing:
            // a model is free only when it reports a zero prompt AND completion
            // rate. Capability ranking (agentic_score) still comes from the
            // external corpus builder, so freshly-classified frees are not
            // ranked here.
            let mut classified_free = 0usize;
            for info in &infos {
                let is_free = info.pricing.as_ref().is_some_and(is_zero_pricing);
                if let Some(entry) = corpus.models.iter_mut().find(|m| m.id == info.id) {
                    if is_free {
                        entry.free = true;
                        entry.paid = false;
                        entry.kind = "chat".into();
                        entry.route_candidate = true;
                        entry.gated_reason = None;
                        classified_free += 1;
                    } else if entry.gated_reason.is_none() && !entry.free {
                        entry.paid = true;
                        entry.gated_reason = Some("paid: non-zero pricing".into());
                    }
                }
            }
            corpus.save(&corpus_path()).context("save corpus")?;
            println!(
                "corpus refreshed: +{} added, -{} retired, {} kept ({} total); {} classified free",
                report.added.len(),
                report.retired.len(),
                report.kept,
                corpus.count,
                classified_free
            );
            println!(
                "note: free/paid is classified from live pricing; capability ranking for \
                 `zoder route` still needs the corpus builder (arena ELO + latency bench)."
            );
            Ok(())
        }
    }
}

// ── passthrough to the zeroclaw binary ───────────────────────────────────────

/// Resolve the `zeroclaw` binary: prefer one next to the running `zoder`
/// (same build), else fall back to PATH.
fn zeroclaw_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("zeroclaw")))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "zeroclaw".to_string())
}

/// Replace this process with `zeroclaw <args>`.
fn passthrough(args: &[String]) -> Result<()> {
    let exe = zeroclaw_bin();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).args(args).exec();
        Err(anyhow::Error::new(err).context(format!("exec {exe}")))
    }
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&exe)
            .args(args)
            .status()
            .context(format!("spawn {exe}"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
