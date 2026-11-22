mod build;
mod gc;
mod naming;
mod record;
mod system;
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
    /// Systems.
    System {
        #[command(subcommand)]
        command: SystemCommands,
    },
    /// Packages.
    Package {
        #[command(subcommand)]
        command: PackageCommands,
    },
    /// The store.
    Store {
        #[command(subcommand)]
        command: StoreCommands,
    },
}

#[derive(Subcommand)]
enum SystemCommands {
    /// Build a system into the store.
    BuildSystem(BuildArgs),
    /// Build a system and write a disk image that boots it.
    BuildImage(ImageArgs),
    /// Build a system and write an archive of its store paths.
    ExportSystem(ExportArgs),
    /// Build a system and switch the running machine to it.
    SwitchSystem(BuildArgs),
    /// Remove old generations of the running machine.
    DeleteGenerations(GenerationArgs),
}

#[derive(Args)]
struct GenerationArgs {
    /// Number of newest generations to keep.
    #[arg(long)]
    keep: usize,
    /// Directory of the generation links.
    #[arg(long, default_value = "/vug/profiles")]
    profiles: PathBuf,
}

#[derive(Args)]
struct GarbageArgs {
    /// Directory of the store (default: as for build-package).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Directory of the generation links.
    #[arg(long, default_value = "/vug/profiles")]
    profiles: PathBuf,
    /// List what would be removed without removing it.
    #[arg(long, overrides_with = "no_dry_run")]
    dry_run: bool,
    /// Remove (after --dry-run).
    #[arg(long, overrides_with = "dry_run")]
    no_dry_run: bool,
}

#[derive(Subcommand)]
enum StoreCommands {
    /// Write the outputs and build keys of the store as a binary cache.
    ExportCache(CacheArgs),
    /// Remove the store paths that no generation needs.
    CollectGarbage(GarbageArgs),
}

#[derive(Args)]
struct CacheArgs {
    /// Directory to write the cache into.
    #[arg(long)]
    directory: PathBuf,
    /// Directory of the store (default: as for build-package).
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Args)]
struct ImageArgs {
    #[command(flatten)]
    build: BuildArgs,
    /// Image file to write.
    #[arg(long)]
    image: PathBuf,
    /// Size of the file system (a number with a k, m, g, or t suffix).
    #[arg(long, default_value = "4g")]
    size: String,
}

#[derive(Args)]
struct ExportArgs {
    #[command(flatten)]
    build: BuildArgs,
    /// Archive file to write.
    #[arg(long)]
    archive: PathBuf,
    /// A system whose store paths are left out (an id or a store path).
    #[arg(long)]
    base: Option<String>,
}

#[derive(Subcommand)]
enum PackageCommands {
    /// Build a package and its dependencies into the store.
    BuildPackage(BuildArgs),
    /// Run a program of a package with the store visible at /vug/store.
    RunPackage(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    build: BuildArgs,
    /// Program to run, relative to the package output.
    #[arg(long)]
    program: String,
    /// Arguments passed to the program.
    #[arg(long, num_args = 0.., allow_hyphen_values = true)]
    arguments: Vec<String>,
}

#[derive(Args)]
struct BuildArgs {
    /// Repository root containing seed.toml and the packages directory.
    #[arg(long)]
    root: PathBuf,
    /// Package name.
    #[arg(long)]
    name: String,
    /// Version to build (default: the greatest version record).
    #[arg(long)]
    package_version: Option<String>,
    /// Directory for the store (default: $XDG_DATA_HOME/vug).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Directory for fetched sources (default: $XDG_CACHE_HOME/vug).
    #[arg(long)]
    cache_dir: Option<PathBuf>,
    /// Parallel build jobs.
    #[arg(long, default_value_t = 1)]
    jobs: u32,
    /// Base URL of a binary cache to take outputs from.
    #[arg(long)]
    binary_cache: Option<String>,
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
const VALUE_OPTIONS: &[&str] = &[
    "--output",
    "--left",
    "--right",
    "--file",
    "--root",
    "--name",
    "--package-version",
    "--data-dir",
    "--cache-dir",
    "--jobs",
    "--program",
    "--arguments",
    "--image",
    "--size",
    "--archive",
    "--base",
    "--binary-cache",
    "--keep",
    "--profiles",
    "--directory",
];

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
        Commands::System { command } => match command {
            SystemCommands::BuildSystem(args) => run_build_system(&cli, args),
            SystemCommands::BuildImage(args) => run_build_image(&cli, args),
            SystemCommands::ExportSystem(args) => run_export_system(&cli, args),
            SystemCommands::DeleteGenerations(args) => run_delete_generations(&cli, args),
            SystemCommands::SwitchSystem(args) => {
                let settings = settings_of(&cli, args)?;
                Err((
                    version::failure_status(),
                    system::switch(&settings, &args.name),
                ))
            }
        },
        Commands::Store { command } => match command {
            StoreCommands::ExportCache(args) => run_export_cache(&cli, args),
            StoreCommands::CollectGarbage(args) => run_collect_garbage(&cli, args),
        },
        Commands::Package { command } => match command {
            PackageCommands::BuildPackage(args) => run_build(&cli, args),
            PackageCommands::RunPackage(args) => run_program(&cli, args),
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
    let mut report = record::check_tree(&args.root);
    let mut systems = 0;
    if let Ok(entries) = std::fs::read_dir(args.root.join("systems")) {
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter_map(|file| file.strip_suffix(".toml").map(str::to_owned))
            .collect();
        names.sort();
        for name in names {
            systems += 1;
            if let Err(message) = system::load(&args.root, &name) {
                report.errors.push(record::RecordError {
                    path: format!("systems/{name}.toml"),
                    message,
                });
            }
        }
    }
    match cli.output {
        Output::Json => print_json(&json!({
            "Packages": report.packages,
            "Versions": report.versions,
            "Systems": systems,
            "Errors": report.errors.iter().map(|e| json!({
                "Path": e.path,
                "Message": e.message,
            })).collect::<Vec<_>>()
        })),
        Output::Text => {
            let mut text = format!(
                "Packages: {}\nVersions: {}\nSystems: {systems}\n",
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
                "Packages\n{}\nVersions\n{}\nSystems\n{systems}\nPath\tMessage\n",
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

fn xdg_dir(variable: &str, home_relative: &str) -> Result<PathBuf, String> {
    match std::env::var_os(variable) {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir).join("vug")),
        _ => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(home_relative).join("vug"))
            .ok_or_else(|| format!("neither {variable} nor HOME is set")),
    }
}

/// Whether this machine runs a vug system, whose store is `/vug/store`.
fn on_vug_system() -> bool {
    Path::new("/vug/store").is_dir()
}

fn default_data_dir() -> Result<PathBuf, String> {
    if on_vug_system() {
        Ok(PathBuf::from("/vug"))
    } else {
        xdg_dir("XDG_DATA_HOME", ".local/share")
    }
}

fn run_delete_generations(cli: &Cli, args: &GenerationArgs) -> Result<i32, (i32, String)> {
    let fail = |e: String| (version::failure_status(), e);
    let removed = gc::delete_generations(&args.profiles, args.keep).map_err(fail)?;
    let refresh = Path::new("/run/current-system/bin/install-boot");
    if !removed.is_empty() && refresh.exists() {
        let status = std::process::Command::new(refresh)
            .status()
            .map_err(|e| fail(format!("{}: {e}", refresh.display())))?;
        if !status.success() {
            return Err(fail(format!("{}: {status}", refresh.display())));
        }
    }
    match cli.output {
        Output::Json => print_json(&json!({ "Removed": removed })),
        Output::Text | Output::Table => {
            write_out(&removed.iter().map(|n| format!("{n}\n")).collect::<String>())
        }
    }?;
    Ok(0)
}

fn run_collect_garbage(cli: &Cli, args: &GarbageArgs) -> Result<i32, (i32, String)> {
    let fail = |e: String| (version::failure_status(), e);
    let data_dir = match &args.data_dir {
        Some(dir) => dir.clone(),
        None => default_data_dir().map_err(fail)?,
    };
    let (kept, removed) =
        gc::collect_garbage(&data_dir, &args.profiles, args.dry_run).map_err(fail)?;
    match cli.output {
        Output::Json => print_json(&json!({
            "Kept": kept,
            "Removed": removed,
            "DryRun": args.dry_run,
        })),
        Output::Text | Output::Table => write_out(
            &removed
                .iter()
                .map(|id| format!("{id}\n"))
                .collect::<String>(),
        ),
    }?;
    Ok(0)
}

fn run_export_cache(cli: &Cli, args: &CacheArgs) -> Result<i32, (i32, String)> {
    let fail = |e: String| (version::failure_status(), e);
    let data_dir = match &args.data_dir {
        Some(dir) => dir.clone(),
        None => default_data_dir().map_err(fail)?,
    };
    let (keys, paths) = build::export_cache(&data_dir, &args.directory).map_err(fail)?;
    match cli.output {
        Output::Json => print_json(&json!({
            "Directory": args.directory,
            "Keys": keys,
            "Written": paths,
        })),
        Output::Text => write_out(&format!("{keys}\n{paths}\n")),
        Output::Table => write_out(&format!("Keys\tWritten\n{keys}\t{paths}\n")),
    }?;
    Ok(0)
}

fn build_package(
    cli: &Cli,
    args: &BuildArgs,
) -> Result<(build::Settings, build::Built), (i32, String)> {
    let fail = |e: String| (version::failure_status(), e);
    let settings = settings_of(cli, args)?;
    let built = build::Builder::new(&settings)
        .and_then(|mut builder| builder.build(&args.name, args.package_version.as_deref()))
        .map_err(fail)?;
    Ok((settings, built))
}

fn settings_of(cli: &Cli, args: &BuildArgs) -> Result<build::Settings, (i32, String)> {
    let fail = |e: String| (version::failure_status(), e);
    let data_dir = match &args.data_dir {
        Some(dir) => dir.clone(),
        None => default_data_dir().map_err(fail)?,
    };
    let cache_dir = match &args.cache_dir {
        Some(dir) => dir.clone(),
        None if on_vug_system() => PathBuf::from("/var/cache/vug"),
        None => xdg_dir("XDG_CACHE_HOME", ".cache").map_err(fail)?,
    };
    if cli.debug {
        eprintln!(
            "root={:?} name={:?} data_dir={data_dir:?} cache_dir={cache_dir:?}",
            args.root, args.name
        );
    }
    let absolute = |dir: &Path| std::path::absolute(dir).map_err(|e| fail(e.to_string()));
    Ok(build::Settings {
        root: absolute(&args.root)?,
        data_dir: absolute(&data_dir)?,
        cache_dir: absolute(&cache_dir)?,
        jobs: args.jobs,
        binary_cache: args.binary_cache.clone(),
    })
}

fn print_system(
    cli: &Cli,
    built: &system::BuiltSystem,
    extra: serde_json::Value,
) -> Result<i32, (i32, String)> {
    let path = format!("/vug/store/{}", built.id);
    match cli.output {
        Output::Json => {
            let mut value = json!({
                "Name": built.name,
                "Path": path,
                "Kernel": built.kernel,
                "StorePaths": built.closure.len(),
            });
            if let (Some(value), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
                value.extend(extra.clone());
            }
            print_json(&value)
        }
        Output::Text => write_out(&format!("{path}\n")),
        Output::Table => write_out(&format!(
            "Name\tPath\tKernel\n{}\t{path}\t{}\n",
            built.name,
            built.kernel.display()
        )),
    }?;
    Ok(0)
}

fn run_build_system(cli: &Cli, args: &BuildArgs) -> Result<i32, (i32, String)> {
    let settings = settings_of(cli, args)?;
    let built = system::build(&settings, &args.name).map_err(|e| (version::failure_status(), e))?;
    print_system(cli, &built, json!({}))
}

fn run_build_image(cli: &Cli, args: &ImageArgs) -> Result<i32, (i32, String)> {
    let settings = settings_of(cli, &args.build)?;
    let built = system::build_image(&settings, &args.build.name, &args.image, &args.size)
        .map_err(|e| (version::failure_status(), e))?;
    print_system(cli, &built, json!({ "Image": args.image }))
}

fn run_export_system(cli: &Cli, args: &ExportArgs) -> Result<i32, (i32, String)> {
    let settings = settings_of(cli, &args.build)?;
    let (built, written) = system::export(
        &settings,
        &args.build.name,
        &args.archive,
        args.base.as_deref(),
    )
    .map_err(|e| (version::failure_status(), e))?;
    print_system(
        cli,
        &built,
        json!({ "Archive": args.archive, "Written": written }),
    )
}

fn run_program(cli: &Cli, args: &RunArgs) -> Result<i32, (i32, String)> {
    let (settings, built) = build_package(cli, &args.build)?;
    let error = build::run_in_store(&settings, &built.id, &args.program, &args.arguments);
    Err((version::failure_status(), error))
}

fn run_build(cli: &Cli, args: &BuildArgs) -> Result<i32, (i32, String)> {
    let (settings, built) = build_package(cli, args)?;
    let path = format!("/vug/store/{}", built.id);
    let host_path = settings.data_dir.join("store").join(&built.id);
    match cli.output {
        Output::Json => print_json(&json!({
            "Name": built.name,
            "Version": built.version,
            "Path": path,
            "HostPath": host_path,
            "Built": built.fresh,
        })),
        Output::Text => write_out(&format!("{path}\n")),
        Output::Table => write_out(&format!(
            "Name\tVersion\tPath\n{}\t{}\t{path}\n",
            built.name, built.version
        )),
    }?;
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
