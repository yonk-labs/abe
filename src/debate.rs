//! Debate engine: broadcast, critique rounds, decision protocols.

use crate::config::{Config, Defaults, Protocol};
use crate::provider::{build_provider, Prompt, Provider};
use crate::report::{judge_prompt, parse_synthesis, synthesis_prompt, Report};
use anyhow::Context;
use futures::future::join_all;
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct RoundAnswer {
    pub model: String,
    pub text: String,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Round {
    pub round: u32,
    pub answers: Vec<RoundAnswer>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebateResult {
    pub final_answer: String,
    pub protocol: String,
    pub report: Report,
    pub models_used: Vec<String>,
    pub rounds: Vec<Round>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Run the full debate: broadcast → critique rounds → decision protocol.
pub async fn run_debate(
    cfg: &Config,
    providers: &[Box<dyn Provider>],
    chairman: &dyn Provider,
    question: &str,
) -> anyhow::Result<DebateResult> {
    let mut warnings: Vec<String> = Vec::new();
    let mut rounds: Vec<Round> = Vec::new();
    // Overall wall-clock budget: a backstop so the debate returns *something*
    // before a caller's tool-call timeout (e.g. an MCP client) gives up on it.
    let deadline = cfg.debate.max_secs.map(|s| Instant::now() + Duration::from_secs(s));

    // Round 0: broadcast the question to all models concurrently.
    let base = base_prompt(cfg, question);
    let round0 = broadcast(providers, &base).await;
    rounds.push(Round {
        round: 0,
        answers: round0,
    });

    // Fault tolerance: require at least `min_models` successful responses.
    let live = latest_successful(&rounds).len() as u32;
    if live < cfg.debate.min_models {
        anyhow::bail!(
            "only {live} model(s) responded successfully; need at least {} (debate.min_models)",
            cfg.debate.min_models
        );
    }

    // Critique rounds 1..=rounds. Two protections against a slow or dead peer
    // stalling the whole debate:
    //   - skip providers that never produced a successful answer; a dead model
    //     would otherwise be re-called (and re-time-out) every single round.
    //   - run the survivors concurrently like the broadcast, so one slow peer
    //     overlaps the others instead of serially adding its latency to theirs.
    for r in 1..=cfg.debate.rounds {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            warnings.push(format!(
                "debate budget (max_secs={}) reached; skipped critique rounds {}..={}",
                cfg.debate.max_secs.unwrap_or(0),
                r,
                cfg.debate.rounds
            ));
            break;
        }
        let latest = latest_successful(&rounds);
        let live: std::collections::HashSet<&str> =
            latest.iter().map(|(n, _)| n.as_str()).collect();
        let answers = join_all(providers.iter().filter(|p| live.contains(p.name())).map(|p| {
            let others = others_labeled(&latest, p.name(), cfg.debate.anonymize);
            let prompt = critique_prompt(cfg, question, &others);
            async move { call_one(p.as_ref(), &prompt).await }
        }))
        .await;
        rounds.push(Round { round: r, answers });
    }

    // Decision protocol over the latest successful answers.
    let latest = latest_successful(&rounds);
    let models_used: Vec<String> = latest.iter().map(|(n, _)| n.clone()).collect();

    let labeled = label_all(&latest, cfg.debate.anonymize);
    let max_bytes = cfg.defaults.max_context_kb as usize * 1024;
    if labeled.len() > max_bytes {
        warnings.push(format!(
            "decision context is ~{}KB, exceeds max_context_kb={}",
            labeled.len() / 1024,
            cfg.defaults.max_context_kb
        ));
    }

    // Chairman candidates: the designated chairman first, then any model that
    // answered successfully this debate, so a down chairman fails over to a live
    // peer instead of collapsing synthesis to a raw first answer.
    let live: Vec<&str> = latest.iter().map(|(n, _)| n.as_str()).collect();
    let mut chairmen: Vec<&dyn Provider> = vec![chairman];
    for p in providers {
        if p.name() != chairman.name() && live.contains(&p.name()) {
            chairmen.push(p.as_ref());
        }
    }

    let (final_answer, report) = match cfg.debate.protocol {
        Protocol::Synthesis => {
            chairman_decide(&chairmen, cfg, synthesis_prompt(question, &labeled), &latest, &mut warnings).await
        }
        Protocol::Judge => {
            chairman_decide(&chairmen, cfg, judge_prompt(question, &labeled), &latest, &mut warnings).await
        }
        Protocol::Majority => majority_decide(&latest),
    };

    Ok(DebateResult {
        final_answer,
        protocol: format!("{:?}", cfg.debate.protocol).to_lowercase(),
        report,
        models_used,
        rounds,
        warnings,
    })
}

/// Build providers from config, resolve the chairman, and run a full debate.
/// Shared by the CLI, MCP, and HTTP surfaces. Apply any rounds/protocol
/// overrides to `cfg` before calling.
pub async fn debate_from_config(cfg: &Config, question: &str) -> anyhow::Result<DebateResult> {
    let providers: Vec<Box<dyn Provider>> = cfg
        .models
        .iter()
        .map(|m| build_provider(m, &cfg.defaults))
        .collect::<anyhow::Result<_>>()?;
    let chair = cfg.resolved_chairman().map(|s| s.to_string());
    let chairman: &dyn Provider = providers
        .iter()
        .find(|p| Some(p.name()) == chair.as_deref())
        .map(|b| b.as_ref())
        .context("chairman model not found among providers")?;
    run_debate(cfg, &providers, chairman, question).await
}

fn base_prompt(cfg: &Config, question: &str) -> Prompt {
    Prompt {
        system: None,
        user: question.to_string(),
        temperature: cfg.defaults.temperature,
        max_tokens: cfg.defaults.max_tokens,
    }
}

async fn broadcast(providers: &[Box<dyn Provider>], prompt: &Prompt) -> Vec<RoundAnswer> {
    join_all(providers.iter().map(|p| call_one(p.as_ref(), prompt))).await
}

async fn call_one(p: &dyn Provider, prompt: &Prompt) -> RoundAnswer {
    match p.complete(prompt).await {
        Ok(a) => RoundAnswer {
            model: a.model_name,
            text: a.text,
            elapsed_ms: a.elapsed_ms,
            error: None,
        },
        Err(e) => RoundAnswer {
            model: p.name().to_string(),
            text: String::new(),
            elapsed_ms: 0,
            error: Some(e.to_string()),
        },
    }
}

/// Most recent successful answer per model, in first-seen order.
fn latest_successful(rounds: &[Round]) -> Vec<(String, String)> {
    let mut names: Vec<String> = Vec::new();
    for rd in rounds {
        for a in &rd.answers {
            if !names.contains(&a.model) {
                names.push(a.model.clone());
            }
        }
    }
    let mut out = Vec::new();
    for name in names {
        for rd in rounds.iter().rev() {
            if let Some(a) = rd
                .answers
                .iter()
                .find(|a| a.model == name && a.error.is_none() && !a.text.is_empty())
            {
                out.push((name.clone(), a.text.clone()));
                break;
            }
        }
    }
    out
}

fn label_all(answers: &[(String, String)], anonymize: bool) -> String {
    answers
        .iter()
        .enumerate()
        .map(|(i, (name, text))| {
            let label = if anonymize {
                format!("Solution {}", (b'A' + i as u8) as char)
            } else {
                name.clone()
            };
            format!("### {label}\n{text}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn others_labeled(answers: &[(String, String)], me: &str, anonymize: bool) -> String {
    let others: Vec<(String, String)> = answers.iter().filter(|(n, _)| n != me).cloned().collect();
    label_all(&others, anonymize)
}

fn critique_prompt(cfg: &Config, question: &str, others: &str) -> Prompt {
    let user = format!(
        "Original question:\n{question}\n\n\
Other participants' current answers:\n{others}\n\n\
Critique the other answers — point out any errors or points of disagreement — then give your own improved, final answer to the original question."
    );
    Prompt {
        system: None,
        user,
        temperature: cfg.defaults.temperature,
        max_tokens: cfg.defaults.max_tokens,
    }
}

/// Minimum token budget for the chairman's reply. The chairman emits more than
/// any single debater — a full merged answer PLUS the agreement/disagreement
/// arrays — so the per-model budget (tuned for one answer) truncates its JSON
/// and silently drops the report. Floor it.
// ponytail: a floor, not a new config knob — add `chairman_max_tokens` only if someone needs to tune it.
const CHAIRMAN_MIN_TOKENS: u32 = 2048;

fn chairman_max_tokens(defaults: &Defaults) -> u32 {
    defaults.max_tokens.max(CHAIRMAN_MIN_TOKENS)
}

/// Synthesis/judge: hand the labeled answers to the first chairman candidate
/// that answers, parse its JSON into (final_answer, report). A down chairman
/// fails over to a live peer (any model that already answered can synthesize);
/// only if every candidate fails do we degrade to the first answer with no
/// report — and always with a warning, so the degradation is never silent.
async fn chairman_decide(
    chairmen: &[&dyn Provider],
    cfg: &Config,
    user: String,
    latest: &[(String, String)],
    warnings: &mut Vec<String>,
) -> (String, Report) {
    let prompt = Prompt {
        system: None,
        user,
        temperature: cfg.defaults.temperature,
        max_tokens: chairman_max_tokens(&cfg.defaults),
    };
    let mut skipped: Vec<String> = Vec::new();
    for (i, ch) in chairmen.iter().enumerate() {
        match ch.complete(&prompt).await {
            Ok(a) => {
                if i > 0 {
                    warnings.push(format!(
                        "chairman fell back to `{}` — skipped: {}",
                        ch.name(),
                        skipped.join("; ")
                    ));
                }
                let (fa, rep, w) = parse_synthesis(&a.text);
                if let Some(w) = w {
                    warnings.push(w);
                }
                return (fa, rep);
            }
            Err(e) => skipped.push(format!("`{}`: {e}", ch.name())),
        }
    }
    warnings.push(format!(
        "all chairman candidates failed ({}); returning the first answer without synthesis",
        skipped.join("; ")
    ));
    (
        latest.first().map(|(_, t)| t.clone()).unwrap_or_default(),
        Report::default(),
    )
}

/// Majority: deterministic clustering by normalized text (no extra LLM call).
/// Picks the largest cluster; reports the count and lists minority answers.
fn majority_decide(latest: &[(String, String)]) -> (String, Report) {
    use std::collections::BTreeMap;
    let total = latest.len();
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, (_, text)) in latest.iter().enumerate() {
        groups.entry(normalize(text)).or_default().push(i);
    }
    let Some(best) = groups.values().max_by_key(|v| v.len()) else {
        return (String::new(), Report::default());
    };
    let rep_idx = best[0];
    let count = best.len();
    let final_answer = latest[rep_idx].1.clone();
    let agreements = vec![format!("{count} of {total} models gave the majority answer")];
    let mut disagreements = Vec::new();
    for members in groups.values() {
        if members[0] != rep_idx {
            disagreements.push(latest[members[0]].1.clone());
        }
    }
    (
        final_answer,
        Report {
            agreements,
            disagreements,
        },
    )
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Estimate total model calls for cost awareness: broadcast + critique rounds,
/// plus one chairman call for synthesis/judge (majority needs none).
pub fn estimate_calls(models: usize, rounds: u32, protocol: Protocol) -> usize {
    let base = models * (rounds as usize + 1);
    let decision = if matches!(protocol, Protocol::Majority) { 0 } else { 1 };
    base + decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::provider::{MockProvider, Provider};

    fn cfg(rounds: u32) -> Config {
        Config::from_yaml(&format!(
            "models: [{{name: a, kind: cli, cli: codex}}, {{name: b, kind: cli, cli: claude}}, {{name: c, kind: cli, cli: opencode}}]\ndebate: {{rounds: {rounds}, protocol: synthesis, chairman: a}}"
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn broadcast_collects_all_answers() {
        let c = cfg(0);
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["ans-a"])),
            Box::new(MockProvider::new("b", ["ans-b"])),
            Box::new(MockProvider::new("c", ["ans-c"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.rounds[0].answers.len(), 3);
        assert_eq!(res.models_used.len(), 3);
        assert_eq!(res.final_answer, "F");
    }

    #[tokio::test]
    async fn synthesis_produces_report() {
        let c = cfg(0);
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["x"])),
            Box::new(MockProvider::new("b", ["y"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"MERGED","agreements":["both agree"],"disagreements":["c"]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.final_answer, "MERGED");
        assert_eq!(res.report.agreements, vec!["both agree"]);
    }

    #[tokio::test]
    async fn one_round_feeds_others_answers_anonymized() {
        let c = cfg(1); // rounds = 1, anonymize defaults true
        let a = MockProvider::new("a", ["a-r0", "a-r1"]);
        let log = a.log_handle();
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(a),
            Box::new(MockProvider::new("b", ["b-r0", "b-r1"])),
            Box::new(MockProvider::new("c", ["c-r0", "c-r1"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        run_debate(&c, &providers, &chair, "Q").await.unwrap();

        let prompts = log.lock().unwrap();
        let round1 = &prompts[1]; // [0] = broadcast, [1] = critique
        assert!(round1.contains("b-r0"), "should include other models' answers");
        assert!(round1.contains("c-r0"));
        assert!(!round1.contains("a-r0"), "should exclude its own answer");
        assert!(round1.contains("Solution A"), "labels should be anonymized");
        assert!(!round1.contains("### b"), "should not label by model name");
    }

    #[tokio::test]
    async fn n_rounds_produces_n_plus_one_round_records() {
        let c = cfg(3);
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["a"])),
            Box::new(MockProvider::new("b", ["b"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.rounds.len(), 4); // round 0 + 3 critique rounds
    }

    #[tokio::test]
    async fn majority_picks_most_common_answer() {
        let mut c = cfg(0);
        c.debate.protocol = crate::config::Protocol::Majority;
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["42"])),
            Box::new(MockProvider::new("b", ["42"])),
            Box::new(MockProvider::new("c", ["7"])),
        ];
        let chair = MockProvider::new("chair", ["unused"]);
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.final_answer, "42");
        assert!(res.report.agreements[0].contains("2 of 3"));
    }

    #[tokio::test]
    async fn judge_uses_chairman_pick() {
        let mut c = cfg(0);
        c.debate.protocol = crate::config::Protocol::Judge;
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["answer-a"])),
            Box::new(MockProvider::new("b", ["answer-b"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"answer-b","agreements":[],"disagreements":["a was weaker"]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.final_answer, "answer-b");
        assert_eq!(res.report.disagreements, vec!["a was weaker"]);
    }

    #[tokio::test]
    async fn aborts_below_min_models() {
        let c = cfg(0); // min_models defaults to 2
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["ok"])),
            Box::new(crate::provider::FailProvider::new("b")),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await;
        assert!(res.is_err(), "only 1 of 2 models succeeded → should abort");
    }

    #[tokio::test]
    async fn drops_failed_model_and_continues() {
        let mut c = cfg(0);
        c.debate.min_models = 2;
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["ok-a"])),
            Box::new(MockProvider::new("b", ["ok-b"])),
            Box::new(crate::provider::FailProvider::new("c")),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.models_used.len(), 2);
        assert!(res.models_used.contains(&"a".to_string()));
        assert!(!res.models_used.contains(&"c".to_string()));
        let r0 = &res.rounds[0];
        assert!(r0.answers.iter().any(|a| a.model == "c" && a.error.is_some()));
    }

    #[tokio::test]
    async fn critique_skips_never_successful_providers() {
        // A provider that fails the broadcast must NOT be re-called in critique
        // rounds — otherwise a dead/timed-out model burns its full timeout every
        // single round, serially stalling the whole debate.
        let c = cfg(1); // one critique round, min_models defaults to 2
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["a0", "a1"])),
            Box::new(MockProvider::new("b", ["b0", "b1"])),
            Box::new(crate::provider::FailProvider::new("c")),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.rounds[0].answers.len(), 3, "broadcast calls every provider");
        let critique = &res.rounds[1];
        assert_eq!(critique.answers.len(), 2, "the dead provider must be skipped in critique");
        assert!(
            critique.answers.iter().all(|a| a.model != "c"),
            "never-successful provider must not be re-called"
        );
    }

    #[tokio::test]
    async fn max_secs_budget_skips_remaining_rounds() {
        // A zero budget must trip immediately: run the broadcast, skip all
        // critique rounds, and still produce a synthesized answer.
        let mut c = cfg(3); // 3 critique rounds requested
        c.debate.max_secs = Some(0);
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["a"])),
            Box::new(MockProvider::new("b", ["b"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert_eq!(res.rounds.len(), 1, "only the broadcast should run under a zero budget");
        assert!(
            res.warnings.iter().any(|w| w.contains("budget")),
            "the budget cut-off must surface as a warning"
        );
        assert_eq!(res.final_answer, "F", "the decision step still runs after the cut-off");
    }

    #[tokio::test]
    async fn chairman_fails_over_to_live_peer() {
        let c = cfg(0); // synthesis protocol, min_models defaults to 2
        let down_chairman = crate::provider::FailProvider::new("a");
        // `b` answers round 0, then can synthesize when promoted to chairman.
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new(
                "b",
                ["b-ans", r#"{"final_answer":"SYNTH-BY-B","agreements":[],"disagreements":[]}"#],
            )),
            Box::new(MockProvider::new("c", ["c-ans"])),
        ];
        let res = run_debate(&c, &providers, &down_chairman, "Q").await.unwrap();
        assert_eq!(res.final_answer, "SYNTH-BY-B", "a down chairman must fail over to a live peer");
        assert!(
            res.warnings.iter().any(|w| w.contains("fell back")),
            "the chairman fallback must surface as a warning"
        );
    }

    #[test]
    fn chairman_gets_token_headroom() {
        use crate::config::Defaults;
        // A tight per-model budget must be floored for the chairman, which must
        // emit a full merged answer PLUS both report arrays in one reply.
        let d = Defaults {
            max_tokens: 512,
            ..Defaults::default()
        };
        assert!(chairman_max_tokens(&d) >= 2048, "tight budget should be floored");
        // A generous budget is respected, never lowered.
        let d = Defaults {
            max_tokens: 4096,
            ..Defaults::default()
        };
        assert_eq!(chairman_max_tokens(&d), 4096);
    }

    #[test]
    fn estimate_calls_counts_rounds_and_decision() {
        use crate::config::Protocol;
        assert_eq!(estimate_calls(3, 2, Protocol::Synthesis), 10); // 3*(2+1) + 1
        assert_eq!(estimate_calls(3, 2, Protocol::Majority), 9); // no chairman call
    }

    #[tokio::test]
    async fn warns_on_oversize_context() {
        let mut c = cfg(0);
        c.defaults.max_context_kb = 0; // force the warning
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("a", ["some answer"])),
            Box::new(MockProvider::new("b", ["another answer"])),
        ];
        let chair = MockProvider::new(
            "chair",
            [r#"{"final_answer":"F","agreements":[],"disagreements":[]}"#],
        );
        let res = run_debate(&c, &providers, &chair, "Q").await.unwrap();
        assert!(res.warnings.iter().any(|w| w.contains("max_context_kb")));
    }
}
