use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
    RolledBack,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DagStep {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Invariants that must hold for the step to be considered correct;
    /// used by the Jev scope gate and the escalation payload.
    pub invariants: Vec<String>,
    /// Files the worker may read or write during this step. Empty = read-only
    /// step (all write tools are denied by the path sandbox).
    #[serde(default)]
    pub allowed_paths: Vec<PathBuf>,
    /// Deterministic gate command run in the worktree on `finish_step`.
    #[serde(default)]
    pub verification_command: Option<String>,
    #[serde(default)]
    pub status: StepStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskDag {
    pub goal: String,
    pub steps: Vec<DagStep>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DagError {
    #[error("DAG must contain at least one step")]
    Empty,
    #[error("duplicate step id '{0}'")]
    DuplicateId(String),
    #[error("step '{0}' depends on unknown step '{1}'")]
    UnknownDependency(String, String),
    #[error("step '{0}' has no invariants")]
    MissingInvariants(String),
    #[error("dependency cycle involving step '{0}'")]
    Cycle(String),
    #[error("goal must not be empty")]
    EmptyGoal,
}

impl TaskDag {
    pub fn validate(&self) -> Result<(), DagError> {
        if self.goal.trim().is_empty() {
            return Err(DagError::EmptyGoal);
        }
        if self.steps.is_empty() {
            return Err(DagError::Empty);
        }
        let ids: BTreeSet<&str> =
            self.steps.iter().map(|s| s.id.as_str()).collect();
        if ids.len() != self.steps.len() {
            let mut seen = BTreeSet::new();
            for step in &self.steps {
                if !seen.insert(step.id.as_str()) {
                    return Err(DagError::DuplicateId(step.id.clone()));
                }
            }
        }
        for step in &self.steps {
            if step.invariants.is_empty()
                || step.invariants.iter().all(|i| i.trim().is_empty())
            {
                return Err(DagError::MissingInvariants(step.id.clone()));
            }
            for dep in &step.depends_on {
                if !ids.contains(dep.as_str()) {
                    return Err(DagError::UnknownDependency(
                        step.id.clone(),
                        dep.clone(),
                    ));
                }
            }
        }
        self.topological_order()?;
        Ok(())
    }

    /// Kahn's algorithm; also serves as cycle detection.
    pub fn topological_order(&self) -> Result<Vec<usize>, DagError> {
        let index: BTreeMap<&str, usize> = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.as_str(), i))
            .collect();
        let mut indegree = vec![0usize; self.steps.len()];
        let mut dependents: Vec<Vec<usize>> =
            vec![Vec::new(); self.steps.len()];
        for (i, step) in self.steps.iter().enumerate() {
            for dep in &step.depends_on {
                if let Some(&j) = index.get(dep.as_str()) {
                    indegree[i] += 1;
                    dependents[j].push(i);
                }
            }
        }
        let mut queue: Vec<usize> = indegree
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| i)
            .collect();
        queue.sort();
        let mut order = Vec::with_capacity(self.steps.len());
        while let Some(i) = queue.first().copied() {
            queue.remove(0);
            order.push(i);
            for &d in &dependents[i] {
                indegree[d] -= 1;
                if indegree[d] == 0 {
                    let pos = queue
                        .iter()
                        .position(|x| *x > d)
                        .unwrap_or(queue.len());
                    queue.insert(pos, d);
                }
            }
        }
        if order.len() != self.steps.len() {
            let stuck = self
                .steps
                .iter()
                .enumerate()
                .find(|(i, _)| indegree[*i] > 0)
                .map(|(_, s)| s.id.clone())
                .unwrap_or_default();
            return Err(DagError::Cycle(stuck));
        }
        Ok(order)
    }

    /// Steps whose dependencies are all completed and that are not started.
    pub fn ready_steps(&self) -> Vec<&DagStep> {
        self.steps
            .iter()
            .filter(|step| step.status == StepStatus::Pending)
            .filter(|step| {
                step.depends_on.iter().all(|dep| {
                    self.steps.iter().any(|s| {
                        &s.id == dep && s.status == StepStatus::Completed
                    })
                })
            })
            .collect()
    }

    pub fn step_mut(&mut self, id: &str) -> Option<&mut DagStep> {
        self.steps.iter_mut().find(|s| s.id == id)
    }

    pub fn step(&self, id: &str) -> Option<&DagStep> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// Compact render for the plan-confirm screen and logs.
    pub fn render_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Goal: {}\n", self.goal));
        let order = self
            .topological_order()
            .unwrap_or((0..self.steps.len()).collect());
        for &i in &order {
            let step = &self.steps[i];
            let deps = if step.depends_on.is_empty() {
                "-".to_string()
            } else {
                step.depends_on.join(",")
            };
            let verify =
                step.verification_command.as_deref().unwrap_or("(none)");
            out.push_str(&format!(
                "  [{}] {} (after {}) verify: {}\n    invariant: {}\n",
                step.id,
                step.description,
                deps,
                verify,
                step.invariants.join(" | ")
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, deps: &[&str]) -> DagStep {
        DagStep {
            id: id.to_string(),
            description: format!("step {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            invariants: vec!["compiles".to_string()],
            allowed_paths: vec![PathBuf::from("src/")],
            verification_command: Some("cargo check".to_string()),
            status: StepStatus::Pending,
        }
    }

    fn dag(steps: Vec<DagStep>) -> TaskDag {
        TaskDag {
            goal: "test goal".to_string(),
            steps,
        }
    }

    #[test]
    fn valid_dag_topo_orders() {
        let d = dag(vec![step("c", &["b"]), step("a", &[]), step("b", &["a"])]);
        assert_eq!(d.validate(), Ok(()));
        let order = d.topological_order().unwrap();
        let names: Vec<&str> =
            order.iter().map(|i| d.steps[*i].id.as_str()).collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("a") < pos("b") && pos("b") < pos("c"));
    }

    #[test]
    fn rejects_cycle_duplicate_and_missing_dep() {
        let d = dag(vec![step("a", &["b"]), step("b", &["a"])]);
        assert!(matches!(d.validate(), Err(DagError::Cycle(_))));

        let d = dag(vec![step("a", &[]), step("a", &[])]);
        assert!(matches!(d.validate(), Err(DagError::DuplicateId(_))));

        let d = dag(vec![step("a", &["ghost"])]);
        assert!(matches!(
            d.validate(),
            Err(DagError::UnknownDependency(_, _))
        ));

        let mut no_inv = step("a", &[]);
        no_inv.invariants = vec![];
        let d = dag(vec![no_inv]);
        assert!(matches!(d.validate(), Err(DagError::MissingInvariants(_))));
    }

    #[test]
    fn ready_steps_respect_completion() {
        let mut d =
            dag(vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])]);
        assert_eq!(d.ready_steps().len(), 1);
        assert_eq!(d.ready_steps()[0].id, "a");
        d.step_mut("a").unwrap().status = StepStatus::Completed;
        assert_eq!(d.ready_steps()[0].id, "b");
        d.step_mut("b").unwrap().status = StepStatus::Failed;
        assert!(d.ready_steps().is_empty(), "failed dep blocks downstream");
    }

    #[test]
    fn dag_json_roundtrip() {
        let d = dag(vec![step("a", &[])]);
        let json = serde_json::to_string(&d).unwrap();
        let back: TaskDag = serde_json::from_str(&json).unwrap();
        assert_eq!(back.steps[0].id, "a");
        assert_eq!(back.steps[0].status, StepStatus::Pending);
    }
}
