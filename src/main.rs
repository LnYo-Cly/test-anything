use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use test_anything::core::bundle::{self, FailureBundles};
use test_anything::core::case::{self, CaseError, LoadedCase};
use test_anything::core::events::EventBus;
use test_anything::core::filter::Filters;
use test_anything::core::output::{self, Format};
use test_anything::core::registry::Registry;
use test_anything::core::report::RunOutcome;
use test_anything::core::runner;
use test_anything::core::schema;
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
        /// Reads variables from a YAML mapping. Repeat for more than one; a
        /// later file overrides an earlier one.
        #[arg(long = "vars-file", value_name = "FILE")]
        vars_files: Vec<PathBuf>,
        /// Only runs cases in one of these projects. Repeat for more than one.
        #[arg(long = "project", value_name = "NAME")]
        projects: Vec<String>,
        /// Only runs cases in one of these suites. Repeat for more than one.
        #[arg(long = "suite", value_name = "NAME")]
        suites: Vec<String>,
        /// Only runs cases in one of these layers. Repeat for more than one.
        #[arg(long = "layer", value_name = "NAME")]
        layers: Vec<String>,
        /// Only runs cases that have one of these tags. Repeat for more than one.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Skips cases that have any of these tags. Repeat for more than one.
        #[arg(long = "skip-tag", value_name = "TAG")]
        skip_tags: Vec<String>,
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
        /// Defines a variable for every case. Repeat for more than one.
        #[arg(long = "var", value_name = "NAME=VALUE", value_parser = parse_var)]
        vars: Vec<(String, String)>,
        /// Reads variables from a YAML mapping. Repeat for more than one; a
        /// later file overrides an earlier one.
        #[arg(long = "vars-file", value_name = "FILE")]
        vars_files: Vec<PathBuf>,
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
            vars_files,
            projects,
            suites,
            layers,
            tags,
            skip_tags,
            bundle_dir,
            no_bundle,
        } => {
            let sources = match VarSources::load(&vars_files, &vars) {
                Ok(sources) => sources,
                Err(message) => return usage_error(&message),
            };
            let filters = Filters {
                project: projects,
                suite: suites,
                layer: layers,
                tag: tags,
                skip_tag: skip_tags,
            };
            run(
                &paths, cli.output, &registry, &sources, &filters, bundle_dir, no_bundle,
            )
        }
        Command::Lint {
            paths,
            vars,
            vars_files,
        } => {
            let sources = match VarSources::load(&vars_files, &vars) {
                Ok(sources) => sources,
                Err(message) => return usage_error(&message),
            };
            lint(&paths, cli.output, &registry, &sources)
        }
        Command::Schema => print_schema(&registry, cli.output),
    }
}

/// Variables that come from outside the case file, in increasing precedence.
#[derive(Debug, Default)]
struct VarSources {
    /// The values of each `--vars-file`, with the file they came from, in
    /// command-line order.
    files: Vec<(PathBuf, Vec<(String, String)>)>,
    /// The values of each `--var`.
    inline: Vec<(String, String)>,
}

impl VarSources {
    /// Reads every `--vars-file` in command-line order.
    ///
    /// The file each value came from is kept so that a problem inside one can
    /// name it; the values are applied in this order, so a later file wins.
    fn load(paths: &[PathBuf], inline: &[(String, String)]) -> Result<Self, String> {
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            files.push((path.clone(), case::load_vars_file(path)?));
        }
        Ok(Self {
            files,
            inline: inline.to_vec(),
        })
    }
}

/// Reports a problem with the command line without running anything.
fn usage_error(message: &str) -> ExitCode {
    let _ = write_stderr(&format!("error: {message}\n"));
    TaExitCode::Usage.into()
}

fn run(
    paths: &[PathBuf],
    format: OutputFormat,
    registry: &Registry,
    sources: &VarSources,
    filters: &Filters,
    bundle_dir: Option<PathBuf>,
    no_bundle: bool,
) -> ExitCode {
    let outcome = execute(paths, registry, sources, filters, bundle_dir, no_bundle);
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
    sources: &VarSources,
    filters: &Filters,
    bundle_dir: Option<PathBuf>,
    no_bundle: bool,
) -> RunOutcome {
    let LoadedCases { cases, errors, .. } = load_cases(paths, sources);
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
    runner::run(&cases, registry, &bus, bundles.as_mut(), filters)
}

/// Validates the cases without running them.
///
/// It shares discovery, loading and [`runner::preflight`] with `ta run`, so the
/// two commands cannot disagree about which cases are valid. Unlike a run, it
/// keeps going after the first bad file and reports every problem at once.
fn lint(
    paths: &[PathBuf],
    format: OutputFormat,
    registry: &Registry,
    sources: &VarSources,
) -> ExitCode {
    let LoadedCases {
        files,
        cases,
        errors,
        ..
    } = load_cases(paths, sources);
    let mut errors = errors;
    errors.extend(runner::preflight(&cases, registry));

    let report = output::LintReport {
        files: files.len(),
        errors,
        cases: cases.iter().map(output::LintCase::new).collect(),
    };
    let rendered = output::render_lint(&report, format.into());
    let _ = write_stdout(&rendered.stdout);
    let _ = write_stderr(&rendered.stderr);

    if report.is_ok() {
        TaExitCode::Success.into()
    } else {
        TaExitCode::InvalidCase.into()
    }
}

/// Writes the schema of the case file format to standard output.
fn print_schema(registry: &Registry, format: OutputFormat) -> ExitCode {
    let value = schema::case_schema(registry);
    let rendered = output::render_schema(&value, format.into());
    let _ = write_stdout(&rendered.stdout);
    let _ = write_stderr(&rendered.stderr);
    TaExitCode::Success.into()
}

/// The files a path list expands to and the cases that loaded from them.
struct LoadedCases {
    files: Vec<PathBuf>,
    cases: Vec<LoadedCase>,
    errors: Vec<CaseError>,
}

/// Discovers, loads and parses every case without running anything.
///
/// A file that does not parse becomes an entry in `errors`; the remaining files
/// still load, so lint can report every problem at once.
fn load_cases(paths: &[PathBuf], sources: &VarSources) -> LoadedCases {
    let files = match case::discover(paths) {
        Ok(files) => files,
        Err(error) => {
            return LoadedCases {
                files: Vec::new(),
                cases: Vec::new(),
                errors: vec![error],
            };
        }
    };
    if files.is_empty() {
        return LoadedCases {
            files,
            cases: Vec::new(),
            errors: vec![CaseError {
                file: paths.first().cloned().unwrap_or_default(),
                message: "no `.yaml` or `.yml` case files found; pass a case file or a directory that contains one".to_string(),
            }],
        };
    }

    let mut cases = Vec::with_capacity(files.len());
    let mut errors = Vec::new();
    // The command line beats the case file, and a later source beats an earlier
    // one: `vars:` first, then each `--vars-file` in order, then `--var`.
    let mut overrides: Vec<(String, String)> = Vec::new();
    for (_, entries) in &sources.files {
        overrides.extend(entries.iter().cloned());
    }
    overrides.extend(sources.inline.iter().cloned());

    for file in &files {
        match case::load_file(file) {
            Ok(mut loaded_case) => {
                // A case that does not use the name ignores it.
                for (name, value) in &overrides {
                    loaded_case.case.vars.insert(name.clone(), value.clone());
                }
                cases.push(loaded_case);
            }
            Err(error) => errors.push(error),
        }
    }

    LoadedCases {
        files,
        cases,
        errors,
    }
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
