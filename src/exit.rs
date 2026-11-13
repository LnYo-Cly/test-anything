//! Process exit codes returned by `ta`.

/// Process exit codes returned by `ta`.
///
/// The numeric values are part of the command-line contract: scripts and
/// agents branch on them, so released values must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// Every requested case ran and passed.
    Success = 0,
    /// At least one case ran and failed an assertion.
    TestFailure = 1,
    /// The command line was invalid, or filters selected no case. clap reports
    /// an invalid command line on its own.
    Usage = 2,
    /// A case file was invalid (validation error).
    InvalidCase = 3,
    /// The environment failed, for example because a target was unreachable.
    Environment = 4,
    /// A case or command exceeded its timeout.
    Timeout = 5,
    /// The requested feature is not implemented yet.
    NotImplemented = 9,
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        std::process::ExitCode::from(code as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode;

    #[test]
    fn numeric_values_match_the_documented_contract() {
        let cases = [
            (ExitCode::Success, 0),
            (ExitCode::TestFailure, 1),
            (ExitCode::Usage, 2),
            (ExitCode::InvalidCase, 3),
            (ExitCode::Environment, 4),
            (ExitCode::Timeout, 5),
            (ExitCode::NotImplemented, 9),
        ];

        for (code, expected) in cases {
            assert_eq!(code as u8, expected, "{code:?}");
        }
    }
}
