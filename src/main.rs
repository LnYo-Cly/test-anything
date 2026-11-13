use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use test_anything::core::bundle::{self, FailureBundles};
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
        /// Overrides a case variable. Repeat for more than one.
        #[arg(long = "var", value_name = "NAME=VALUE", value_parser = parse_var)]
        vars: Vec<(String, String)>,
        /// Directory that receives one subdirectory per failed case.
        #[arg(long, value_name = "DIRECTORY")]
        bundle_dir: Option<PathBuf>,
        /// Do not write failure bundles.
        #[arg(long)]
        no_bundle: bool,
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

/// Splits a `--var name=value` argument.
fn parse_var(text: &str) -> Result<(String, String), String> {
    let Some((name, value)) = text.split_once('=') else {
        return Err(format!("`{text}` has no `=`; write `--var name=value`"));
    };
    if name.is_empty() {
        return Err(format!(
            "the name is empty in `{text}`; write `--var name=value`"
        ));
    }
    Ok((name.to_string(), value.to_string()))
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut registry = Registry::new();
    if let Err(message) = drivers::register(&mut registry) {
        let _ = write_stderr(&format!("ta: internal error: {message}\n"));
        return TaExitCode::Environment.into();
    }

    match cli.command {
        Command::Run {
            paths,
            vars,
            bundle_dir,
            no_bundle,
        } => run(&paths, cli.output, &registry, &vars, bundle_dir, no_bundle),
        Command::Lint { .. } => not_implemented("lint", cli.output),
        Command::Schema => not_implemented("schema", cli.output),
    }
}

fn run(
    paths: &[PathBuf],
    format: OutputFormat,
    registry: &Registry,
    vars: &[(String, String)],
    bundle_dir: Option<PathBuf>,
    no_bundle: bool,
) -> ExitCode {
    let outcome = execute(paths, registry, vars, bundle_dir, no_bundle);
    let rendered = output::render(&outcome, format.into());
    // A write error must not replace the run's exit code. The common case is a
    // reader that exits early, such as `head` or `jq`; there is usually nowhere
    // left to report the failure either.
    let _ = write_stdout(&rendered.stdout);
    let _ = write_stderr(&rendered.stderr);
    outcome.exit_code().into()
}

fn execute(
    paths: &[PathBuf],
    registry: &Registry,
    vars: &[(String, String)],
    bundle_dir: Option<PathBuf>,
    no_bundle: bool,
) -> RunOutcome {
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
            Ok(mut loaded_case) => {
                // Command-line variables beat the ones in the case file, and
                // beat nothing else: a case that does not use the name ignores it.
                for (name, value) in vars {
                    loaded_case.case.vars.insert(name.clone(), value.clone());
                }
                loaded.push(loaded_case);
            }
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return RunOutcome::Rejected { errors };
    }

    let mut bundles = if no_bundle {
        None
    } else {
        Some(FailureBundles::new(
            bundle_dir.unwrap_or_else(|| PathBuf::from(bundle::DEFAULT_DIRECTORY)),
        ))
    };

    let bus = EventBus::new();
    runner::run(&loaded, registry, &bus, bundles.as_mut())
}

fn not_implemented(name: &str, format: OutputFormat) -> ExitCode {
    match format {
        OutputFormat::Text => {
            let _ = write_stderr(&format!("ta {name}: not implemented yet\n"));
        }
        OutputFormat::Json => {
            let _ = write_stdout(&format!(
                "{}\n",
                json!({
                    "ok": false,
                    "error": {
                        "code": "not_implemented",
                        "message": format!("ta {name} is not implemented yet"),
                    },
                })
            ));
        }
    }
    TaExitCode::NotImplemented.into()
}

/// Writes `text` to standard output, returning a write error instead of
/// panicking.
///
/// `print!` panics when the reader of the pipe exits early, which replaces the
/// run's exit code with 101 and hides the test result.
fn write_stdout(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()
}

/// Writes `text` to standard error, returning a write error instead of
/// panicking for the same reason as [`write_stdout`].
fn write_stderr(text: &str) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    stderr.write_all(text.as_bytes())?;
    stderr.flush()
}
