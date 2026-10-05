use std::collections::BTreeMap;

use serde_json::{Value, json};

/// A typed judgment request. `instructions` and criteria entries are arbitrary
/// JSON (string, object, or array) per the System One wire format.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoulCriteria {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no: Option<Value>,
}

/// Ordered (sorted-key) question set; the ordering keeps cache keys canonical.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QuestionSet {
    pub questions: BTreeMap<String, Question>,
}

impl QuestionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(mut self, name: impl Into<String>, question: Question) -> Self {
        self.questions.insert(name.into(), question);
        self
    }

    pub fn len(&self) -> usize {
        self.questions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}

/// Builds the AiWrangler-style `{question, focus}` instruction object.
pub fn instructions(question: &str, focus: &str) -> Value {
    json!({ "question": question, "focus": focus })
}

/// P(yes) judgment. `yes`/`no` describe each arm of the criteria.
pub fn noul(instructions: Value, yes: Option<Value>, no: Option<Value>) -> Question {
    Question::Noul {
        instructions,
        criteria: Some(NoulCriteria { yes, no }),
    }
}

/// Single-label judgment with a full probability distribution.
pub fn choice(instructions: Value, criteria: BTreeMap<String, Value>) -> Question {
    Question::Choice {
        instructions,
        criteria,
    }
}

/// Rubric judgment over ordinal levels (0..=n-1); requires at least 2 levels.
pub fn score(instructions: Value, levels: Vec<Value>) -> Result<Question, String> {
    if levels.len() < 2 {
        return Err("score questions require at least 2 rubric levels".to_string());
    }
    Ok(Question::Score {
        instructions,
        criteria: levels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_wire_format_roundtrip() {
        let set = QuestionSet::new()
            .add(
                "in_scope",
                noul(
                    instructions("Is the edit in scope?", "The invariant governs."),
                    Some(json!("It stays within the step.")),
                    Some(json!("It strays beyond the step.")),
                ),
            )
            .add(
                "triage",
                choice(
                    instructions("Pick a verdict.", "Choose exactly one."),
                    BTreeMap::from([
                        ("syntax_fix".to_string(), json!("Compiler error fix")),
                        ("deadlock".to_string(), json!("Architectural block")),
                    ]),
                ),
            )
            .add(
                "novelty",
                score(
                    instructions("How novel?", "Compare to prior failures."),
                    vec![json!("identical"), json!("novel")],
                )
                .unwrap(),
            );

        let wire = serde_json::to_value(&set).unwrap();
        assert_eq!(wire["questions"]["in_scope"]["type"], "noul");
        assert_eq!(wire["questions"]["in_scope"]["criteria"]["yes"], "It stays within the step.");
        assert_eq!(wire["questions"]["triage"]["type"], "choice");
        assert_eq!(wire["questions"]["novelty"]["criteria"].as_array().unwrap().len(), 2);

        let back: QuestionSet = serde_json::from_value(wire).unwrap();
        assert_eq!(back, set);
    }

    #[test]
    fn score_requires_two_levels() {
        assert!(score(instructions("q", "f"), vec![json!("only")]).is_err());
    }
}
