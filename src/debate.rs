//! Debate engine: broadcast, critique rounds, decision protocols.

use crate::config::{Config, Protocol};
use crate::provider::{Prompt, Provider};
use crate::report::{parse_synthesis, synthesis_prompt, Report};
use futures::future::join_all;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RoundAnswer {
    pub model: String,
    pub text: String,
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

    // Round 0: broadcast the question to all models concurrently.
    let base = base_prompt(cfg, question);
    let round0 = broadcast(providers, &base).await;
    rounds.push(Round {
        round: 0,
        answers: round0,
    });

    // Critique rounds 1..=rounds.
    for r in 1..=cfg.debate.rounds {
        let latest = latest_successful(&rounds);
        let mut answers = Vec::with_capacity(providers.len());
        for p in providers {
            let others = others_labeled(&latest, p.name(), cfg.debate.anonymize);
            let prompt = critique_prompt(cfg, question, &others);
            answers.push(call_one(p.as_ref(), &prompt).await);
        }
        rounds.push(Round { round: r, answers });
    }

    // Decision protocol over the latest successful answers.
    let latest = latest_successful(&rounds);
    let models_used: Vec<String> = latest.iter().map(|(n, _)| n.clone()).collect();

    let (final_answer, report) = match cfg.debate.protocol {
        Protocol::Synthesis => {
            let labeled = label_all(&latest, cfg.debate.anonymize);
            let prompt = Prompt {
                system: None,
                user: synthesis_prompt(question, &labeled),
                temperature: cfg.defaults.temperature,
                max_tokens: cfg.defaults.max_tokens,
                json_mode: true,
            };
            match chairman.complete(&prompt).await {
                Ok(a) => {
                    let (fa, rep, w) = parse_synthesis(&a.text);
                    if let Some(w) = w {
                        warnings.push(w);
                    }
                    (fa, rep)
                }
                Err(e) => {
                    warnings.push(format!("chairman failed: {e}"));
                    (
                        latest.first().map(|(_, t)| t.clone()).unwrap_or_default(),
                        Report::default(),
                    )
                }
            }
        }
        Protocol::Majority | Protocol::Judge => {
            anyhow::bail!("protocol not yet implemented in v0.1 (synthesis only)")
        }
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

fn base_prompt(cfg: &Config, question: &str) -> Prompt {
    Prompt {
        system: None,
        user: question.to_string(),
        temperature: cfg.defaults.temperature,
        max_tokens: cfg.defaults.max_tokens,
        json_mode: false,
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
            error: None,
        },
        Err(e) => RoundAnswer {
            model: p.name().to_string(),
            text: String::new(),
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
        json_mode: false,
    }
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
}
