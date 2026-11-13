//! Result types shared by the runner, the event bus and the renderer.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::core::assert::AssertionReport;
use crate::core::case::CaseError;
use crate::exit::ExitCode;

/// The state of a finished case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStatus {
    /// Every step ran and every assertion passed.
    Passed,
    /// An assertion did not pass.
    Failed,
    /// A step could not run to completion.
    Error,
}

impl CaseStatus {
    /// The name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }
}

/// The state of a finished step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// The step ran and every assertion passed.
    Passed,
    /// The step ran and an assertion did not pass.
    Failed,
    /// The step could not run to completion.
    Error,
    /// The case stopped before this step.
    Skipped,
}

impl StepStatus {
    /// The name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Skipped => "skipped",
        }
    }
}

/// The category of a problem, which decides the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// An assertion did not pass.
    Assertion,
    /// The case or a step's arguments are invalid.
    InvalidInput,
    /// The environment failed, for example an unreachable target.
    Environment,
    /// A step exceeded its time budget.
    Timeout,
    /// The step ran but could not finish its work.
    Execution,
}

impl ProblemKind {
    /// The name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assertion => "assertion",
            Self::InvalidInput => "invalid_input",
            Self::Environment => "environment",
            Self::Timeout => "timeout",
            Self::Execution => "execution",
        }
    }
}

/// What went wrong and why it matters.
#[derive(Debug, Clone)]
pub struct Problem {
    /// Category that decides the exit code.
    pub kind: ProblemKind,
    /// Message that tells the user what happened.
    pub message: String,
}

/// One step of a case report.
#[derive(Debug, Clone)]
pub struct StepReport {
    /// Registered step name.
    pub name: String,
    /// Step state.
    pub status: StepStatus,
    /// How long the step took.
    pub duration: Duration,
    /// The JSON result, when the step produced one.
    pub result: Option<Value>,
    /// Every assertion evaluated for this step.
    pub assertions: Vec<AssertionReport>,
    /// The problem that stopped the case, if this step caused it.
    pub error: Option<Problem>,
}

/// One case of a run.
#[derive(Debug, Clone)]
pub struct CaseReport {
    /// File the case came from.
    pub file: PathBuf,
    /// Case name.
    pub name: String,
    /// Case state.
    pub status: CaseStatus,
    /// How long the case took.
    pub duration: Duration,
    /// Every step, including the ones skipped after a failure.
    pub steps: Vec<StepReport>,
    /// The problem that made the case fail, if any.
    pub error: Option<Problem>,
    /// Failure bundle directory, when one was written for this case.
    pub bundle: Option<PathBuf>,
}

/// Every case of a run.
#[derive(Debug, Clone)]
pub struct RunReport {
    /// Case reports in execution order.
    pub cases: Vec<CaseReport>,
    /// How long the run took.
    pub duration: Duration,
    /// Failure bundles that could not be written.
    ///
    /// A bundle is evidence for the reader, not part of the verdict, so these
    /// are reported without changing the run's exit code.
    pub bundle_errors: Vec<String>,
}

/// Counts derived from a [`RunReport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    /// Number of cases.
    pub cases: usize,
    /// Cases that passed.
    pub cases_passed: usize,
    /// Cases that failed an assertion.
    pub cases_failed: usize,
    /// Cases that could not run to completion.
    pub cases_errored: usize,
    /// Number of steps, including skipped ones.
    pub steps: usize,
    /// Steps that passed.
    pub steps_passed: usize,
    /// Steps that failed an assertion.
    pub steps_failed: usize,
    /// Steps that could not run to completion.
    pub steps_errored: usize,
    /// Steps skipped after a failure.
    pub steps_skipped: usize,
}

impl RunReport {
    /// Derives the counts reported to the user.
    pub fn summary(&self) -> Summary {
        let mut summary = Summary {
            cases: self.cases.len(),
            ..Summary::default()
        };
        for case in &self.cases {
            match case.status {
                CaseStatus::Passed => summary.cases_passed += 1,
                CaseStatus::Failed => summary.cases_failed += 1,
                CaseStatus::Error => summary.cases_errored += 1,
            }
            for step in &case.steps {
                summary.steps += 1;
                match step.status {
                    StepStatus::Passed => summary.steps_passed += 1,
                    StepStatus::Failed => summary.steps_failed += 1,
                    StepStatus::Error => summary.steps_errored += 1,
                    StepStatus::Skipped => summary.steps_skipped += 1,
                }
            }
        }
        summary
    }

    fn problem_kinds(&self) -> Vec<ProblemKind> {
        let mut kinds = Vec::new();
        for case in &self.cases {
            if let Some(problem) = &case.error {
                kinds.push(problem.kind);
            }
            for step in &case.steps {
                if let Some(problem) = &step.error {
                    kinds.push(problem.kind);
                }
            }
        }
        kinds
    }
}

/// What `ta run` produced.
#[derive(Debug, Clone)]
pub enum RunOutcome {
    /// Nothing ran because at least one case is invalid.
    Rejected {
        /// One entry per invalid case file.
        errors: Vec<CaseError>,
    },
    /// Every case ran.
    Completed(RunReport),
}

impl RunOutcome {
    /// The exit code for this outcome.
    ///
    /// When several problems occur in one run the most fundamental one wins:
    /// invalid input, then environment, then timeout, then a failed assertion.
    pub fn exit_code(&self) -> ExitCode {
        let RunOutcome::Completed(report) = self else {
            return ExitCode::InvalidCase;
        };
        let kinds = report.problem_kinds();
        if kinds.contains(&ProblemKind::InvalidInput) {
            return ExitCode::InvalidCase;
        }
        if kinds.contains(&ProblemKind::Environment) {
            return ExitCode::Environment;
        }
        if kinds.contains(&ProblemKind::Timeout) {
            return ExitCode::Timeout;
        }
        if !kinds.is_empty() {
            return ExitCode::TestFailure;
        }
        ExitCode::Success
    }

    /// Whether every case ran and passed.
    pub fn is_ok(&self) -> bool {
        self.exit_code() == ExitCode::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_with(kind: ProblemKind) -> RunReport {
        RunReport {
            cases: vec![CaseReport {
                file: PathBuf::from("case.yaml"),
                name: "case".to_string(),
                status: CaseStatus::Error,
                duration: Duration::ZERO,
                steps: vec![StepReport {
                    name: "test.step".to_string(),
                    status: StepStatus::Error,
                    duration: Duration::ZERO,
                    result: None,
                    assertions: Vec::new(),
                    error: Some(Problem {
                        kind,
                        message: "problem".to_string(),
                    }),
                }],
                error: Some(Problem {
                    kind,
                    message: "problem".to_string(),
                }),
                bundle: None,
            }],
            duration: Duration::ZERO,
            bundle_errors: Vec::new(),
        }
    }

    #[test]
    fn exit_code_prefers_invalid_input_over_other_problems() {
        let mut report = report_with(ProblemKind::Environment);
        report
            .cases
            .push(report_with(ProblemKind::Timeout).cases.remove(0));
        report
            .cases
            .push(report_with(ProblemKind::InvalidInput).cases.remove(0));

        assert_eq!(
            RunOutcome::Completed(report).exit_code(),
            ExitCode::InvalidCase
        );
    }

    #[test]
    fn exit_code_prefers_environment_over_timeout_and_assertion() {
        let mut report = report_with(ProblemKind::Assertion);
        report
            .cases
            .push(report_with(ProblemKind::Timeout).cases.remove(0));
        report
            .cases
            .push(report_with(ProblemKind::Environment).cases.remove(0));

        assert_eq!(
            RunOutcome::Completed(report).exit_code(),
            ExitCode::Environment
        );
    }

    #[test]
    fn exit_code_prefers_timeout_over_a_failed_assertion() {
        let mut report = report_with(ProblemKind::Assertion);
        report
            .cases
            .push(report_with(ProblemKind::Timeout).cases.remove(0));

        assert_eq!(RunOutcome::Completed(report).exit_code(), ExitCode::Timeout);
    }

    #[test]
    fn an_empty_report_is_a_success() {
        let report = RunReport {
            cases: Vec::new(),
            duration: Duration::ZERO,
            bundle_errors: Vec::new(),
        };

        assert_eq!(RunOutcome::Completed(report).exit_code(), ExitCode::Success);
    }
}
