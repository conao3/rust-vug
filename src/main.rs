mod version;

use std::io::{Read as _, Write as _};

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
}

#[derive(Subcommand)]
enum VersionCommands {
    /// Compare two version strings.
    CompareVersions(CompareArgs),
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

/// Options that consume their own value: a trailing `help` after one of these
/// is the value, not a help request.
const VALUE_OPTIONS: &[&str] = &["--output", "--left", "--right"];

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

fn run(cli: Cli) -> Result<i32, (i32, String)> {
    let Commands::Version { command } = cli.command;
    let VersionCommands::CompareVersions(args) = command;
    let left = resolve(&args.left).map_err(|e| (version::failure_status(), e))?;
    let right = resolve(&args.right).map_err(|e| (version::failure_status(), e))?;
    let ordering = version::compare(&left, &right);
    if cli.debug {
        eprintln!("left={left:?} right={right:?} ordering={ordering:?}");
    }
    let name = ordering_name(ordering);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match cli.output {
        Output::Json => writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&json!({ "Ordering": name }))
                .map_err(|e| (version::other_status(), e.to_string()))?
        ),
        Output::Text => writeln!(out, "{name}"),
        Output::Table => writeln!(out, "Ordering\n{name}"),
    }
    .map_err(|e| (version::other_status(), e.to_string()))?;
    Ok(0)
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
