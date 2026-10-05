pub mod client;
pub mod questions;

pub use client::{
    JevClient, JevConfig, JevError, JevResult, JevUsage, UnavailableReason,
};
pub use questions::{Question, QuestionSet, choice, instructions, noul, score};

use std::collections::BTreeMap;

/// A single answer from Jev, keyed by question type.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevValue {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Score {
        score: f64,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
}

impl JevValue {
    /// P(yes) for noul answers; panics on other variants (use the typed accessors).
    pub fn as_noul(&self) -> Option<f64> {
        match self {
            JevValue::Noul { noul } => Some(*noul),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<(&str, f64)> {
        match self {
            JevValue::Choice {
                choice,
                confidence,
                probabilities: _,
            } => Some((choice.as_str(), *confidence)),
            _ => None,
        }
    }

    pub fn as_score(&self) -> Option<(f64, f64)> {
        match self {
            JevValue::Score {
                score,
                confidence,
                probabilities: _,
            } => Some((*score, *confidence)),
            _ => None,
        }
    }

    /// Score probabilities keyed by numeric level string (wire format),
    /// e.g. `"0" -> 0.83`. Keys are validated to be unsigned integers.
    pub fn score_probabilities(&self) -> Option<&BTreeMap<String, f64>> {
        match self {
            JevValue::Score { probabilities, .. } => Some(probabilities),
            _ => None,
        }
    }

    /// Validates wire bounds: probabilities in [0,1], scores non-negative, no NaN.
    pub fn sanitize(&mut self) -> Result<(), String> {
        let ok = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        match self {
            JevValue::Noul { noul } => {
                if !ok(*noul) {
                    return Err(format!(
                        "noul probability out of range: {noul}"
                    ));
                }
            }
            JevValue::Choice {
                choice,
                confidence,
                probabilities,
            } => {
                if !ok(*confidence) {
                    return Err(format!(
                        "choice '{choice}' confidence out of range: {confidence}"
                    ));
                }
                if probabilities.is_empty() {
                    return Err(format!(
                        "choice '{choice}' has empty probabilities"
                    ));
                }
                for (label, p) in probabilities.iter() {
                    if !ok(*p) {
                        return Err(format!(
                            "choice '{choice}' probability '{label}' out of range: {p}"
                        ));
                    }
                }
            }
            JevValue::Score {
                score,
                confidence,
                probabilities,
            } => {
                if !score.is_finite() || *score < 0.0 {
                    return Err(format!("score out of range: {score}"));
                }
                if !ok(*confidence) {
                    return Err(format!(
                        "score confidence out of range: {confidence}"
                    ));
                }
                for (level, p) in probabilities.iter() {
                    if level.parse::<u8>().is_err() {
                        return Err(format!(
                            "score level key '{level}' is not an integer"
                        ));
                    }
                    if !ok(*p) {
                        return Err(format!(
                            "score level {level} probability out of range: {p}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
