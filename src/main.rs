mod naming;
mod record;
mod version;

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;
use version::model::Ordering;

/// Declarative package manager and system builder.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Output::Json)]
    output: Output,
    /// Print the parsed inputs to stderr.
    #[arg(long, global = true, overrides_with = "no_debug")]
    debug: bool,
    /// Disable debug output (after --debug).
    #[arg(long, global = true, overrides_with = "debug")]
    no_debug: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, ValueEnum)]
enum Output {
    Json,
    Text,
    Table,
}

#[derive(Subcommand)]
enum Commands {
    /// Version handling.
    Version {
        #[command(subcommand)]
        command: VersionCommands,
    },
    /// Package records.
    Record {
        #[command(subcommand)]
        command: RecordCommands,
    },
}

#[derive(Subcommand)]
enum VersionCommands {
    /// Compare two version strings.
    CompareVersions(CompareArgs),
}

#[derive(Subcommand)]
enum RecordCommands {
    /// Print the canonical form of a record file.
    FormatRecord(FormatArgs),
    /// Check that a record file is valid and canonical.
    CheckRecord(FileArgs),
    /// Check every record under DIR/packages.
    CheckTree(RootArgs),
}

#[derive(Args)]
struct CompareArgs {
    /// Left-hand version string (or file://PATH).
    #[arg(long)]
    left: String,
    /// Right-hand version string (or file://PATH).
    #[arg(long)]
    right: String,
}

#[derive(Args)]
struct FormatArgs {
    /// Record file to format.
    #[arg(long)]
    file: PathBuf,
    /// Rewrite the file when it is not canonical.
    #[arg(long, overrides_with = "no_write")]
    write: bool,
    /// Do not rewrite the file (after --write).
    #[arg(long, overrides_with = "write")]
    no_write: bool,
}

#[derive(Args)]
struct FileArgs {
    /// Record file to check.
    #[arg(long)]
    file: PathBuf,
}

#[derive(Args)]
struct RootArgs {
    /// Repository root containing the packages directory.
    #[arg(long)]
    root: PathBuf,
}

/// Options that consume their own value: a trailing `help` after one of these
/// is the value, not a help request.
const VALUE_OPTIONS: &[&str] = &["--output", "--left", "--right", "--file", "--root"];

/// A bare `help` at the end of the arguments asks for the help of the command
/// reached so far. clap only installs a `help` subcommand where a subcommand
/// exists, so translate it for leaf commands as well.
fn normalize_args(mut args: Vec<String>) -> Vec<String> {
    if args.last().is_some_and(|a| a == "help")
        && args
            .get(args.len().saturating_sub(2))
            .is_none_or(|a| !VALUE_OPTIONS.contains(&a.as_str()))
        && let Some(last) = args.last_mut()
    {
        *last = "--help".to_owned();
    }
    args
}

fn resolve(value: &str) -> Result<String, String> {
    let Some(path) = value.strip_prefix("file://") else {
        return Ok(value.to_owned());
    };
    if path == "/dev/stdin" {
        let mut data = String::new();
        std::io::stdin()
            .read_to_string(&mut data)
            .map_err(|e| format!("{path}: {e}"))?;
        Ok(data)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
    }
}

fn ordering_name(ordering: Ordering) -> &'static str {
    match ordering {
        Ordering::Lt => "Less",
        Ordering::Eq => "Equal",
        Ordering::Gt => "Greater",
    }
}

fn write_out(text: &str) -> Result<(), (i32, String)> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| (version::other_status(), e.to_string()))
}

fn print_json(value: &serde_json::Value) -> Result<(), (i32, String)> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| (version::other_status(), e.to_string()))?;
    write_out(&format!("{text}\n"))
}

fn print_errors(errors: &[record::RecordError], output: Output) -> Result<(), (i32, String)> {
    match output {
        Output::Json => print_json(&json!({
            "Errors": errors.iter().map(|e| json!({
                "Path": e.path,
                "Message": e.message,
            })).collect::<Vec<_>>()
        })),
        Output::Text => {
            let mut text = String::new();
            for e in errors {
                text.push_str(&e.path);
                text.push_str(": ");
                text.push_str(&e.message);
                text.push('\n');
            }
            write_out(&text)
        }
        Output::Table => {
            let mut text = String::from("Path\tMessage\n");
            for e in errors {
                text.push_str(&e.path);
                text.push('\t');
                text.push_str(&e.message);
                text.push('\n');
            }
            write_out(&text)
        }
    }
}

fn run(cli: Cli) -> Result<i32, (i32, String)> {
    match &cli.command {
        Commands::Version { command } => {
            let VersionCommands::CompareVersions(args) = command;
            run_compare(&cli, args)
        }
        Commands::Record { command } => match command {
            RecordCommands::FormatRecord(args) => run_format(&cli, args),
            RecordCommands::CheckRecord(args) => run_check_record(&cli, args),
            RecordCommands::CheckTree(args) => run_check_tree(&cli, args),
        },
    }
}

fn run_compare(cli: &Cli, args: &CompareArgs) -> Result<i32, (i32, String)> {
    let left = resolve(&args.left).map_err(|e| (version::failure_status(), e))?;
    let right = resolve(&args.right).map_err(|e| (version::failure_status(), e))?;
    let ordering = version::compare(&left, &right);
    if cli.debug {
        eprintln!("left={left:?} right={right:?} ordering={ordering:?}");
    }
    let name = ordering_name(ordering);
    match cli.output {
        Output::Json => print_json(&json!({ "Ordering": name })),
        Output::Text => write_out(&format!("{name}\n")),
        Output::Table => write_out(&format!("Ordering\n{name}\n")),
    }?;
    Ok(0)
}

/// The name of the directory holding `path`, for the name-is-directory rule.
fn dir_name(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_owned)
}

fn run_format(cli: &Cli, args: &FormatArgs) -> Result<i32, (i32, String)> {
    if cli.debug {
        eprintln!("file={:?} write={:?}", args.file, args.write);
    }
    let check = match record::check_file(&args.file, dir_name(&args.file).as_deref()) {
        Ok(c) => c,
        Err(errors) => {
            print_errors(&errors, cli.output)?;
            return Ok(version::failure_status());
        }
    };
    if args.write && check.changed {
        std::fs::write(&args.file, &check.canonical).map_err(|e| {
            (
                version::failure_status(),
                format!("{}: {e}", args.file.display()),
            )
        })?;
    }
    match cli.output {
        Output::Json => print_json(&json!({
            "Changed": check.changed,
            "Record": check.canonical,
        })),
        Output::Text => write_out(&check.canonical),
        Output::Table => write_out(&format!("Changed\n{}\n{}", check.changed, check.canonical)),
    }?;
    Ok(0)
}

fn run_check_record(cli: &Cli, args: &FileArgs) -> Result<i32, (i32, String)> {
    if cli.debug {
        eprintln!("file={:?}", args.file);
    }
    let check = match record::check_file(&args.file, dir_name(&args.file).as_deref()) {
        Ok(c) => c,
        Err(errors) => {
            print_errors(&errors, cli.output)?;
            return Ok(version::failure_status());
        }
    };
    let mut errors = Vec::new();
    if check.changed {
        errors.push(record::RecordError {
            path: args.file.display().to_string(),
            message: "not in canonical form".to_owned(),
        });
    }
    print_errors(&errors, cli.output)?;
    Ok(if errors.is_empty() {
        0
    } else {
        version::failure_status()
    })
}

fn run_check_tree(cli: &Cli, args: &RootArgs) -> Result<i32, (i32, String)> {
    if cli.debug {
        eprintln!("root={:?}", args.root);
    }
    let report = record::check_tree(&args.root);
    match cli.output {
        Output::Json => print_json(&json!({
            "Packages": report.packages,
            "Versions": report.versions,
            "Errors": report.errors.iter().map(|e| json!({
                "Path": e.path,
                "Message": e.message,
            })).collect::<Vec<_>>()
        })),
        Output::Text => {
            let mut text = format!(
                "Packages: {}\nVersions: {}\n",
                report.packages, report.versions
            );
            for e in &report.errors {
                text.push_str(&e.path);
                text.push_str(": ");
                text.push_str(&e.message);
                text.push('\n');
            }
            write_out(&text)
        }
        Output::Table => {
            let mut text = format!(
                "Packages\n{}\nVersions\n{}\nPath\tMessage\n",
                report.packages, report.versions
            );
            for e in &report.errors {
                text.push_str(&e.path);
                text.push('\t');
                text.push_str(&e.message);
                text.push('\n');
            }
            write_out(&text)
        }
    }?;
    Ok(if report.errors.is_empty() {
        0
    } else {
        version::failure_status()
    })
}

fn main() {
    let cli = Cli::try_parse_from(normalize_args(std::env::args().collect())).unwrap_or_else(|e| {
        let success = matches!(
            e.kind(),
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
        );
        let _ = e.print();
        std::process::exit(if success { 0 } else { version::usage_status() });
    });
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err((code, error)) => {
            eprintln!("{error}");
            std::process::exit(code);
        }
    }
}
