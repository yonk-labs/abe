//! Agreement/disagreement report + synthesis prompt builder + JSON parsing.
//!
//! The report is a *synthesized interpretation* (a reader aid), never ground
//! truth — raw per-model answers are always preserved in the DebateResult.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub agreements: Vec<String>,
    #[serde(default)]
    pub disagreements: Vec<String>,
}

#[derive(Deserialize)]
struct SynthesisJson {
    final_answer: String,
    #[serde(default)]
    agreements: Vec<String>,
    #[serde(default)]
    disagreements: Vec<String>,
}

/// Parse the chairman's reply into (final_answer, report, optional warning).
/// Tolerant: extracts the first `{...}` block; on failure returns the raw text
/// as the final answer with an empty report and a warning.
pub fn parse_synthesis(text: &str) -> (String, Report, Option<String>) {
    if let Some(json) = extract_json_object(text) {
        if let Ok(s) = serde_json::from_str::<SynthesisJson>(&json) {
            return (
                s.final_answer,
                Report {
                    agreements: s.agreements,
                    disagreements: s.disagreements,
                },
                None,
            );
        }
    }
    (
        text.trim().to_string(),
        Report::default(),
        Some("chairman did not return parseable JSON; using raw text as final answer".to_string()),
    )
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}

/// Build the judge's instruction (user content): pick the single best answer.
pub fn judge_prompt(question: &str, labeled_answers: &str) -> String {
    format!(
        "You are an impartial judge of a panel of AI models. Below is a user question and each model's answer.\n\n\
Question:\n{question}\n\n\
Model answers:\n{labeled_answers}\n\n\
Score each answer for correctness and clarity, then SELECT THE SINGLE BEST answer (verbatim) as the final answer. Also note where models agreed and disagreed.\n\
Respond with ONLY a JSON object (no prose, no markdown fences) in exactly this shape:\n\
{{\"final_answer\": \"<the best answer, verbatim>\", \"agreements\": [\"<point of agreement>\"], \"disagreements\": [\"<point of disagreement>\"]}}"
    )
}

/// Build the chairman's synthesis instruction (user content).
pub fn synthesis_prompt(question: &str, labeled_answers: &str) -> String {
    format!(
        "You are the chairman of a panel of AI models. Below is a user question and each model's answer.\n\n\
Question:\n{question}\n\n\
Model answers:\n{labeled_answers}\n\n\
Produce a single best final answer that merges the strongest reasoning, and identify where the models agreed and where they disagreed.\n\
Respond with ONLY a JSON object (no prose, no markdown fences) in exactly this shape:\n\
{{\"final_answer\": \"<merged best answer>\", \"agreements\": [\"<point all/most models agreed on>\"], \"disagreements\": [\"<point where models differed>\"]}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json() {
        let (final_answer, report, warn) = parse_synthesis(
            r#"{"final_answer":"FINAL","agreements":["a1","a2"],"disagreements":["d1"]}"#,
        );
        assert_eq!(final_answer, "FINAL");
        assert_eq!(report.agreements, vec!["a1", "a2"]);
        assert_eq!(report.disagreements, vec!["d1"]);
        assert!(warn.is_none());
    }

    #[test]
    fn extracts_json_from_noisy_text() {
        let (final_answer, _r, warn) = parse_synthesis(
            "Sure!\n{\"final_answer\":\"X\",\"agreements\":[],\"disagreements\":[]}\nhope that helps",
        );
        assert_eq!(final_answer, "X");
        assert!(warn.is_none());
    }

    #[test]
    fn falls_back_on_unparseable() {
        let (final_answer, report, warn) = parse_synthesis("totally not json");
        assert_eq!(final_answer, "totally not json");
        assert!(report.agreements.is_empty());
        assert!(warn.is_some());
    }
}
