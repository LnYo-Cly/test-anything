use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use test_anything::core::case::{self, CaseError};
use test_anything::core::events::EventBus;
use test_anything::core::output::{self, Format};
use test_anything::core::registry::Registry;
use test_anything::core::report::RunOutcome;
use test_anything::core::runner;
use test_anything::drivers;
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
    /// Human-readable results on standard output, diagnostics on standard error.
    Text,
    /// A single JSON object on standard output.
    Json,
}

impl From<OutputFormat> for Format {
    fn from(format: OutputFormat) -> Self {
        match format {
            OutputFormat::Text => Self::Text,
            OutputFormat::Json => Self::Json,
        }
    }
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

    let mut registry = Registry::new();
    if let Err(message) = drivers::register(&mut registry) {
        eprintln!("ta: internal error: {message}");
        return TaExitCode::Environment.into();
    }

    match cli.command {
        Command::Run { paths } => run(&paths, cli.output, &registry),
        Command::Lint { .. } => not_implemented("lint", cli.output),
        Command::Schema => not_implemented("schema", cli.output),
    }
}

fn run(paths: &[PathBuf], format: OutputFormat, registry: &Registry) -> ExitCode {
    let outcome = execute(paths, registry);
    let rendered = output::render(&outcome, format.into());
    print!("{}", rendered.stdout);
    eprint!("{}", rendered.stderr);
    outcome.exit_code().into()
}

fn execute(paths: &[PathBuf], registry: &Registry) -> RunOutcome {
    let files = match case::discover(paths) {
        Ok(files) => files,
        Err(error) => {
            return RunOutcome::Rejected {
                errors: vec![error],
            };
        }
    };
    if files.is_empty() {
        return RunOutcome::Rejected {
            errors: vec![CaseError {
                file: paths.first().cloned().unwrap_or_default(),
                message: "no `.yaml` or `.yml` case files found; pass a case file or a directory that contains one".to_string(),
            }],
        };
    }

    let mut loaded = Vec::with_capacity(files.len());
    let mut errors = Vec::new();
    for file in files {
        match case::load_file(&file) {
            Ok(loaded_case) => loaded.push(loaded_case),
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return RunOutcome::Rejected { errors };
    }

    let bus = EventBus::new();
    runner::run(&loaded, registry, &bus)
}

fn not_implemented(name: &str, format: OutputFormat) -> ExitCode {
    match format {
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
