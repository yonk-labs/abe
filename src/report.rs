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

/// Markers that tell a judge-style prompt the caller has already run a
/// deterministic verify gate (build/tests) and it PASSED — bob embeds a
/// `## VERIFY OUTPUT` section in its judge statements and only invokes the
/// judge on gate-passing diffs. Case-insensitive substring match.
const GATE_PASS_MARKERS: &[&str] = &[
    "verify output",
    "verify passed",
    "verify gate passed",
    "gate passed",
    "deterministic gate",
];

/// Calibration block appended to judge-style prompts when the input shows a
/// passing deterministic gate. Without it, judges re-litigate spec wording
/// and completeness-by-literal-reading, turning gate-passing diffs into
/// fail/uncertain verdicts (observed 2026-09: bob runs forced into
/// RepeatedUncertain on spec-lawyering).
const GATE_PASSED_CALIBRATION: &str = "\n\nCALIBRATION: the input shows a deterministic verify gate (build/tests) has already PASSED for this change. That gate — not you — is the authority on build and test behavior. You may block ONLY on a demonstrated correctness or safety defect visible in the diff itself: never on spec interpretation, wording, scope/completeness by literal reading, or style. If you find no such defect, return pass — or uncertain with concrete reasons if the information is genuinely insufficient — but never fail on speculation or wording disputes.";

/// Return the calibration block when any of the given texts carries
/// gate-pass language, else an empty string. Shared by the judge/validate
/// prompt builders so all judge surfaces get the same conditioning.
pub(crate) fn gate_calibration_for(texts: &[&str]) -> &'static str {
    let hit = texts.iter().any(|t| {
        let t = t.to_lowercase();
        GATE_PASS_MARKERS.iter().any(|m| t.contains(m))
    });
    if hit {
        GATE_PASSED_CALIBRATION
    } else {
        ""
    }
}

/// Build the judge's instruction (user content): pick the single best answer.
pub fn judge_prompt(question: &str, labeled_answers: &str) -> String {
    let calibration = gate_calibration_for(&[question]);
    format!(
        "You are an impartial judge of a panel of AI models. Below is a user question and each model's answer.\n\n\
Question:\n{question}\n\n\
Model answers:\n{labeled_answers}\n\n\
Score each answer for correctness and clarity, then SELECT THE SINGLE BEST answer (verbatim) as the final answer. Also note where models agreed and disagreed.\n\
Respond with ONLY a JSON object (no prose, no markdown fences) in exactly this shape:\n\
{{\"final_answer\": \"<the best answer, verbatim>\", \"agreements\": [\"<point of agreement>\"], \"disagreements\": [\"<point of disagreement>\"]}}{calibration}"
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

    #[test]
    fn judge_prompt_calibrates_when_gate_passed() {
        // Bob's debate-mode judge statements embed "## VERIFY OUTPUT".
        let p = judge_prompt("spec vs diff\n\n## VERIFY OUTPUT\nok", "a: answer");
        assert!(p.contains("CALIBRATION"), "gate-pass language must trigger calibration");
        assert!(p.contains("never fail on speculation"));
    }

    #[test]
    fn judge_prompt_untouched_without_gate_language() {
        let p = judge_prompt("which answer is best?", "a: answer");
        assert!(!p.contains("CALIBRATION"));
    }

    #[test]
    fn gate_calibration_detection_is_case_insensitive() {
        assert!(!gate_calibration_for(&["## Verify Output\n1 passed"]).is_empty());
        assert!(!gate_calibration_for(&["verify PASSED"]).is_empty());
        assert!(gate_calibration_for(&["nothing about gates here"]).is_empty());
    }
}
