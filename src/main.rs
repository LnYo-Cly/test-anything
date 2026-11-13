use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use test_anything::exit::ExitCode as TaExitCode;

/// An agent-friendly, cross-layer testing tool.
#[derive(Debug, Parser)]
#[command(name = "ta", version, about)]
struct Cli {
    /// How command results are reported.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,

    #[command(subcommand)]
    command: Command,
}

/// Output format selected with `--output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable messages on standard error.
    Text,
    /// A single JSON object on standard output.
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run test cases from the given files or directories.
    Run {
        /// Case files or directories to run.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Validate case files without running them.
    Lint {
        /// Case files or directories to validate.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Print the JSON Schema for the case file format.
    Schema,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let name = match &cli.command {
        Command::Run { .. } => "run",
        Command::Lint { .. } => "lint",
        Command::Schema => "schema",
    };

    match cli.output {
        OutputFormat::Text => {
            eprintln!("ta {name}: not implemented yet");
        }
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": {
                        "code": "not_implemented",
                        "message": format!("ta {name} is not implemented yet"),
                    },
                })
            );
        }
    }

    TaExitCode::NotImplemented.into()
}
