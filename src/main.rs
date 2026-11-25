mod build;
mod fhs;
mod gc;
mod home;
mod merge;
mod naming;
mod provenance;
mod record;
mod runtime_nix;
mod secret;
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
    /// User environment records.
    Home {
        #[command(subcommand)]
        command: HomeCommands,
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
    /// Nix binary cache outputs.
    NixCache {
        #[command(subcommand)]
        command: NixCacheCommands,
    },
    /// The store.
    Store {
        #[command(subcommand)]
        command: StoreCommands,
    },
}

#[derive(Subcommand)]
enum NixCacheCommands {
    /// Verify and stage the reference closure from cache.nixos.org.
    FetchClosure(FetchClosureArgs),
}

#[derive(Args)]
struct FetchClosureArgs {
    /// Absolute Nix store paths to resolve.
    #[arg(long, required = true, num_args = 1..)]
    store_path: Vec<String>,
    /// Existing directory in which to create a staging directory.
    #[arg(long)]
    directory: String,
}

#[derive(Subcommand)]
enum SystemCommands {
    #[command(hide = true)]
    PrepareSecret {
        #[arg(long)]
        name: String,
        #[arg(long)]
        uid: u32,
    },
    /// Build a system into the store.
    BuildSystem(BuildArgs),
    /// Build a system and write a disk image that boots it.
    BuildImage(ImageArgs),
    /// Plan or apply a system installation on a whole disk.
    #[cfg(target_os = "linux")]
    Install(InstallArgs),
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
    #[command(flatten)]
    mode: GarbageModeArgs,
}

#[derive(Args)]
struct GarbageModeArgs {
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
    /// List signed cache objects reachable from the current package records.
    ListCache(CacheInventoryArgs),
    /// Remove the store paths that no generation needs.
    CollectGarbage(GarbageArgs),
    /// Make a key pair for signing a binary cache.
    GenerateKey,
}

#[derive(Args)]
struct CacheInventoryArgs {
    /// Repository root containing seed.toml and the packages directory.
    #[arg(long)]
    root: PathBuf,
    /// URL of a signed cache.
    #[arg(long)]
    binary_cache: String,
    /// Public key used to verify cache metadata.
    #[arg(long)]
    trusted_key: String,
}

#[derive(Args)]
struct CacheArgs {
    /// Directory to write the cache into.
    #[arg(long)]
    directory: PathBuf,
    /// Directory of the store (default: as for build-package).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// File holding the secret key that signs the build keys.
    #[arg(long)]
    secret_key_file: Option<PathBuf>,
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

#[cfg(target_os = "linux")]
#[derive(Args)]
struct InstallArgs {
    #[command(flatten)]
    build: BuildArgs,
    /// Target whole block disk.
    #[arg(long)]
    disk: PathBuf,
    /// Overwrite the target with the planned system.
    #[arg(long, overrides_with = "no_apply")]
    apply: bool,
    /// Show the plan without writing (after --apply).
    #[arg(long, overrides_with = "apply")]
    no_apply: bool,
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
    /// Name to build. Package commands accept name:output.
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
    /// Read SHA-256 verified sources and patches exclusively from this directory.
    #[arg(long)]
    source_cache_dir: Option<PathBuf>,
    /// Parallel build jobs.
    #[arg(long, default_value_t = 1)]
    jobs: u32,
    /// Package to build locally when its output is absent from the local index (repeatable).
    #[arg(long)]
    build_from_source: Vec<String>,
    #[command(flatten)]
    cache: CacheSourceArgs,
}

#[derive(Args)]
struct CacheSourceArgs {
    /// Base URL of a binary cache to take outputs from.
    #[arg(long)]
    binary_cache: Option<String>,
    /// Public key whose signature makes the cache's ids for build keys trusted.
    #[arg(long)]
    trusted_key: Option<String>,
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
    #[command(about = "Classify a candidate diff and compute a merge decision")]
    DecideMerge(MergeArgs),
    #[command(about = "Verify source URLs, hashes and patch provenance for a candidate tree")]
    VerifyProvenance(ProvenanceArgs),
}

#[derive(Args)]
struct ProvenanceArgs {
    #[arg(long, help = "Git repository containing the candidate objects")]
    root: PathBuf,
    #[arg(long, help = "Versioned JSON request (or file://PATH)")]
    input: String,
    #[arg(long, help = "Trusted JSON provenance policy (or file://PATH)")]
    policy: String,
    #[arg(long, help = "Write verified sources and patches by SHA-256")]
    source_cache_dir: Option<PathBuf>,
}

#[derive(Args)]
struct MergeArgs {
    #[arg(long, help = "Versioned JSON request (or file://PATH)")]
    input: String,
}

#[derive(Subcommand)]
enum HomeCommands {
    /// Print the canonical form of a home record.
    FormatHome(FormatArgs),
    /// Print the canonical form of a home fragment.
    FormatFragment(FormatArgs),
    /// Compose a home record and its selected fragments.
    ComposeHome(ComposeHomeArgs),
    /// Build a home output from installed or cached packages and record a generation.
    BuildHome(BuildHomeArgs),
    /// Run a program from a home output in its store namespace.
    RunProgram(RunHomeProgramArgs),
    /// Prepare credentials for the active home in the current login.
    PrepareSecrets,
    /// Activate a built generation in HOME.
    SwitchHome(SwitchHomeArgs),
    /// Activate the nearest earlier retained generation.
    RollbackHome(HomeDirs),
    /// Delete older home generations.
    DeleteGenerations(HomeGenerationArgs),
    /// Remove user-store paths that no home generation needs.
    CollectGarbage(HomeGarbageArgs),
}

#[derive(Args)]
struct HomeDirs {
    /// Directory holding the user store and its index.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Directory holding the home generations and profile lock.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[derive(Args)]
struct BuildHomeArgs {
    #[command(flatten)]
    home: ComposeHomeArgs,
    #[command(flatten)]
    dirs: HomeDirs,
    #[command(flatten)]
    cache: CacheSourceArgs,
}

#[derive(Args)]
struct RunHomeProgramArgs {
    #[arg(long)]
    home_output: PathBuf,
    #[arg(long)]
    program: PathBuf,
    #[arg(last = true)]
    arguments: Vec<std::ffi::OsString>,
}

#[derive(Args)]
struct SwitchHomeArgs {
    #[command(flatten)]
    dirs: HomeDirs,
    /// Number of the built generation to activate.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    generation: u64,
}

#[derive(Args)]
struct HomeGenerationArgs {
    #[command(flatten)]
    dirs: HomeDirs,
    /// Number of newest generations to keep, with a minimum of one.
    #[arg(long)]
    keep: usize,
}

#[derive(Args)]
struct HomeGarbageArgs {
    #[command(flatten)]
    dirs: HomeDirs,
    #[command(flatten)]
    mode: GarbageModeArgs,
}

#[derive(Args)]
struct ComposeHomeArgs {
    /// Repository root containing homes and home-fragments.
    #[arg(long)]
    root: PathBuf,
    /// Home record name.
    #[arg(long)]
    name: String,
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
    "--input",
    "--policy",
    "--root",
    "--name",
    "--package-version",
    "--data-dir",
    "--state-dir",
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
    "--secret-key-file",
    "--trusted-key",
    "--profiles",
    "--directory",
    "--store-path",
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
            RecordCommands::DecideMerge(args) => run_decide_merge(&cli, args),
            RecordCommands::VerifyProvenance(args) => run_verify_provenance(&cli, args),
        },
        Commands::Home { command } => match command {
            HomeCommands::FormatHome(args) => run_format_with(&cli, args, |path| {
                home::check_file(path, home::Kind::Home).map_err(|error| vec![error])
            }),
            HomeCommands::FormatFragment(args) => run_format_with(&cli, args, |path| {
                home::check_file(path, home::Kind::Fragment).map_err(|error| vec![error])
            }),
            HomeCommands::ComposeHome(args) => run_compose_home(&cli, args),
            HomeCommands::BuildHome(args) => run_build_home(&cli, args),
            HomeCommands::RunProgram(args) => {
                home::run::run(&args.home_output, &args.program, &args.arguments)
                    .map_err(input_failure)?;
                Ok(0)
            }
            HomeCommands::PrepareSecrets => {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| input_failure("HOME is not set".to_owned()))?;
                home::activate::prepare_secrets(Path::new(&home)).map_err(input_failure)?;
                Ok(0)
            }
            HomeCommands::SwitchHome(args) => run_activate_home(
                &cli,
                &args.dirs,
                home::activate::Selection::Generation(args.generation),
            ),
            HomeCommands::RollbackHome(dirs) => {
                run_activate_home(&cli, dirs, home::activate::Selection::Previous)
            }
            HomeCommands::DeleteGenerations(args) => {
                let (state, data) = args.dirs.paths().map_err(input_failure)?;
                let removed = home::output::delete_generations(&state, &data, args.keep)
                    .map_err(input_failure)?;
                print_deleted_generations(&cli, &removed)
            }
            HomeCommands::CollectGarbage(args) => {
                let (state, data) = args.dirs.paths().map_err(input_failure)?;
                let (kept, removed) =
                    home::output::collect_garbage(&state, &data, args.mode.dry_run)
                        .map_err(input_failure)?;
                print_garbage(&cli, kept, &removed, args.mode.dry_run)
            }
        },
        Commands::System { command } => match command {
            SystemCommands::PrepareSecret { name, uid } => {
                system::secrets::prepare(name, *uid).map_err(input_failure)?;
                Ok(0)
            }
            SystemCommands::BuildSystem(args) => run_build_system(&cli, args),
            SystemCommands::BuildImage(args) => run_build_image(&cli, args),
            #[cfg(target_os = "linux")]
            SystemCommands::Install(args) => run_install(&cli, args),
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
        Commands::NixCache { command } => {
            let NixCacheCommands::FetchClosure(args) = command;
            run_fetch_closure(&cli, args)
        }
        Commands::Store { command } => match command {
            StoreCommands::ExportCache(args) => run_export_cache(&cli, args),
            StoreCommands::ListCache(args) => run_list_cache(&cli, args),
            StoreCommands::CollectGarbage(args) => run_collect_garbage(&cli, args),
            StoreCommands::GenerateKey => {
                let (secret, public) =
                    vug::sign::generate().map_err(|e| (version::failure_status(), e))?;
                match cli.output {
                    Output::Json => print_json(&json!({ "Secret": secret, "Public": public })),
                    Output::Text | Output::Table => write_out(&format!("{secret}\n{public}\n")),
                }?;
                Ok(0)
            }
        },
        Commands::Package { command } => match command {
            PackageCommands::BuildPackage(args) => run_build(&cli, args),
            PackageCommands::RunPackage(args) => run_program(&cli, args),
        },
    }
}

fn run_fetch_closure(cli: &Cli, args: &FetchClosureArgs) -> Result<i32, (i32, String)> {
    use vug::nix_cache::{Cache, Limits, StorePath};

    let roots = args
        .store_path
        .iter()
        .map(|path| resolve(path).and_then(|path| path.parse::<StorePath>()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(input_failure)?;
    let directory = resolve(&args.directory).map_err(input_failure)?;
    if cli.debug {
        eprintln!("store_path={roots:?} directory={directory:?}");
    }

    let cache = Cache::default();
    let closure = cache
        .resolve(&roots, Limits::default())
        .map_err(input_failure)?;
    let staging = cache
        .fetch(&closure, Path::new(&directory))
        .map_err(input_failure)?;
    let staging_path = staging
        .path()
        .to_str()
        .ok_or_else(|| input_failure("staging path must be UTF-8".to_owned()))?;
    match cli.output {
        Output::Json => print_json(&json!({
            "Directory": staging_path,
            "Paths": closure.entries().iter().map(|info| json!({
                "StorePath": info.store_path().as_str(),
                "NarHash": info.nar_hash(),
                "NarSize": info.nar_size(),
                "FileSize": info.file_size(),
                "References": info.references().iter().map(StorePath::as_str).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })),
        Output::Text => write_out(&format!("{staging_path}\n")),
        Output::Table => {
            let mut text = format!("Directory\n{staging_path}\nStorePath\tNarHash\tNarSize\n");
            for info in closure.entries() {
                text.push_str(&format!(
                    "{}\t{}\t{}\n",
                    info.store_path(),
                    info.nar_hash(),
                    info.nar_size()
                ));
            }
            write_out(&text)
        }
    }?;
    let _ = staging.keep();
    Ok(0)
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

fn run_decide_merge(cli: &Cli, args: &MergeArgs) -> Result<i32, (i32, String)> {
    let input = resolve(&args.input).map_err(|e| (version::failure_status(), e))?;
    let report = merge::evaluate(&input).map_err(|e| (version::failure_status(), e))?;
    match cli.output {
        Output::Json => {
            let value = serde_json::to_value(&report)
                .map_err(|e| (version::other_status(), e.to_string()))?;
            print_json(&value)
        }
        Output::Text => write_out(&format!("{}\n", report.decision)),
        Output::Table => write_out(&format!("Decision\n{}\n", report.decision)),
    }?;
    Ok(0)
}

fn run_verify_provenance(cli: &Cli, args: &ProvenanceArgs) -> Result<i32, (i32, String)> {
    let input_failure = |e| (version::failure_status(), e);
    let input = resolve(&args.input).map_err(input_failure)?;
    let policy = resolve(&args.policy).map_err(input_failure)?;
    let report = provenance::verify(
        &args.root,
        &input,
        &policy,
        args.source_cache_dir.as_deref(),
    )
    .map_err(input_failure)?;
    match cli.output {
        Output::Json => print_json(
            &serde_json::to_value(report).map_err(|e| (version::other_status(), e.to_string()))?,
        ),
        Output::Text => write_out(&report.summary(" ")),
        Output::Table => write_out(&format!("Check\tTarget\tStatus\n{}", report.summary("\t"))),
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
    run_format_with(cli, args, |path| {
        record::check_file(path, dir_name(path).as_deref())
    })
}

fn run_format_with(
    cli: &Cli,
    args: &FormatArgs,
    check_file: impl FnOnce(&Path) -> Result<record::FileCheck, Vec<record::RecordError>>,
) -> Result<i32, (i32, String)> {
    if cli.debug {
        eprintln!("file={:?} write={:?}", args.file, args.write);
    }
    let check = match check_file(&args.file) {
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

fn run_compose_home(cli: &Cli, args: &ComposeHomeArgs) -> Result<i32, (i32, String)> {
    if cli.debug {
        eprintln!("root={:?} name={:?}", args.root, args.name);
    }
    let record = match home::load(&args.root, &args.name) {
        Ok(record) => record,
        Err(error) => {
            print_errors(&[error], cli.output)?;
            return Ok(version::failure_status());
        }
    };
    let canonical = record
        .canonical()
        .map_err(|e| (version::other_status(), e.to_string()))?;
    match cli.output {
        Output::Json => print_json(&json!({ "Record": canonical })),
        Output::Text => write_out(&canonical),
        Output::Table => write_out(&format!("Record\n{canonical}")),
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

fn input_failure(error: String) -> (i32, String) {
    (version::failure_status(), error)
}

impl HomeDirs {
    fn paths(&self) -> Result<(PathBuf, PathBuf), String> {
        let resolve = |explicit: &Option<PathBuf>, variable, home_relative| match explicit {
            Some(path) => Ok(path.clone()),
            None => xdg_dir(variable, home_relative).map(|path| path.join("home")),
        };
        Ok((
            resolve(&self.state_dir, "XDG_STATE_HOME", ".local/state")?,
            resolve(&self.data_dir, "XDG_DATA_HOME", ".local/share")?,
        ))
    }
}

/// Settings of a machine that runs a vug system: `binary-cache` and
/// `trusted-key`, used where the command line gives none.
const MACHINE_CONFIG: &str = "/etc/vug.toml";

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
    print_deleted_generations(cli, &removed)
}

fn print_deleted_generations(cli: &Cli, removed: &[u64]) -> Result<i32, (i32, String)> {
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
        gc::collect_garbage(&data_dir, &args.profiles, args.mode.dry_run).map_err(fail)?;
    print_garbage(cli, kept, &removed, args.mode.dry_run)
}

fn print_garbage(
    cli: &Cli,
    kept: usize,
    removed: &[String],
    dry_run: bool,
) -> Result<i32, (i32, String)> {
    match cli.output {
        Output::Json => print_json(&json!({
            "Kept": kept,
            "Removed": removed,
            "DryRun": dry_run,
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

fn run_list_cache(cli: &Cli, args: &CacheInventoryArgs) -> Result<i32, (i32, String)> {
    let settings = build::Settings {
        root: args.root.clone(),
        data_dir: PathBuf::new(),
        cache_dir: PathBuf::new(),
        source_cache_dir: None,
        jobs: 1,
        binary_cache: Some(args.binary_cache.clone()),
        trusted_key: Some(args.trusted_key.clone()),
    };
    let objects = build::list_cache(&settings).map_err(input_failure)?;
    match cli.output {
        Output::Json => print_json(&json!(objects)),
        Output::Text | Output::Table => write_out(
            &objects
                .iter()
                .map(|object| format!("{object}\n"))
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
    let secret = match &args.secret_key_file {
        None => None,
        Some(file) => Some(
            std::fs::read_to_string(file)
                .map_err(|e| fail(format!("read {}: {e}", file.display())))?
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned(),
        ),
    };
    let (keys, paths) =
        build::export_cache(&data_dir, &args.directory, secret.as_deref()).map_err(fail)?;
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
        .and_then(|mut builder| {
            builder.build_from_source(&args.build_from_source)?;
            builder.build(&args.name, args.package_version.as_deref())
        })
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
    let source_cache_dir = args.source_cache_dir.as_deref().map(absolute).transpose()?;
    let (binary_cache, trusted_key) = if source_cache_dir.is_some() {
        if args.cache.binary_cache.is_some() || args.cache.trusted_key.is_some() {
            return Err(fail(
                "--source-cache-dir cannot be combined with a binary cache".into(),
            ));
        }
        (None, None)
    } else {
        cache_source(&args.cache)?
    };
    Ok(build::Settings {
        root: absolute(&args.root)?,
        data_dir: absolute(&data_dir)?,
        cache_dir: absolute(&cache_dir)?,
        source_cache_dir,
        jobs: args.jobs,
        binary_cache,
        trusted_key,
    })
}

fn cache_source(args: &CacheSourceArgs) -> Result<(Option<String>, Option<String>), (i32, String)> {
    let (mut binary_cache, mut trusted_key) = (args.binary_cache.clone(), args.trusted_key.clone());
    if on_vug_system()
        && let Ok(text) = std::fs::read_to_string(MACHINE_CONFIG)
    {
        let table = text
            .parse::<toml::Table>()
            .map_err(|e| input_failure(format!("{MACHINE_CONFIG}: {e}")))?;
        let value = |key: &str| table.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        binary_cache = binary_cache.or_else(|| value("binary-cache"));
        trusted_key = trusted_key.or_else(|| value("trusted-key"));
    }
    Ok((binary_cache, trusted_key))
}

fn run_build_home(cli: &Cli, args: &BuildHomeArgs) -> Result<i32, (i32, String)> {
    let (state, data_dir) = args.dirs.paths().map_err(input_failure)?;
    let (binary_cache, trusted_key) = cache_source(&args.cache)?;
    let settings = build::Settings {
        root: args.home.root.clone(),
        data_dir,
        cache_dir: PathBuf::new(),
        source_cache_dir: None,
        jobs: 1,
        binary_cache,
        trusted_key,
    };
    let built = home::output::build(&settings, &state, &args.home.name).map_err(input_failure)?;
    match cli.output {
        Output::Json => print_json(&json!({
            "Name": args.home.name,
            "Id": built.id,
            "Path": built.path,
            "Generation": built.generation,
        })),
        Output::Text => write_out(&format!("{}\n", built.path.display())),
        Output::Table => write_out(&format!(
            "Name\tGeneration\tPath\n{}\t{}\t{}\n",
            args.home.name,
            built.generation,
            built.path.display()
        )),
    }?;
    Ok(0)
}

fn run_activate_home(
    cli: &Cli,
    dirs: &HomeDirs,
    selection: home::activate::Selection,
) -> Result<i32, (i32, String)> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| input_failure("HOME is not set".to_owned()))?;
    let (state, data) = dirs.paths().map_err(input_failure)?;
    let (number, units_pending) =
        home::activate::activate(&state, &data, Path::new(&home), selection)
            .map_err(input_failure)?;
    if units_pending && !matches!(cli.output, Output::Json) {
        eprintln!("user units will be loaded on the next login");
    }
    match cli.output {
        Output::Json => print_json(&json!({ "Generation": number, "UnitsPending": units_pending })),
        Output::Text => write_out(&format!("{number}\n")),
        Output::Table => write_out(&format!("Generation\n{number}\n")),
    }?;
    Ok(0)
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

#[cfg(target_os = "linux")]
fn run_install(cli: &Cli, args: &InstallArgs) -> Result<i32, (i32, String)> {
    let settings = settings_of(cli, &args.build)?;
    let plan = system::install::run(&settings, &args.build.name, &args.disk, args.apply)
        .map_err(input_failure)?;
    match cli.output {
        Output::Json => print_json(&plan),
        Output::Table => {
            let mut rows = String::from("Disk\tBytes\tExistingTable\tApplied\n");
            rows.push_str(&format!(
                "{}\t{}\t{}\t{}\n\nName\tBytes\tType\tMount\n",
                args.disk.display(),
                plan["Bytes"],
                plan["ExistingTable"].as_str().unwrap_or_default(),
                plan["Applied"],
            ));
            if let Some(partitions) = plan["Partitions"].as_array() {
                for partition in partitions {
                    rows.push_str(&format!(
                        "{}\t{}\t{}\t{}\n",
                        partition["Name"].as_str().unwrap_or_default(),
                        partition["Bytes"],
                        partition["Type"].as_str().unwrap_or_default(),
                        partition["Mount"].as_str().unwrap_or_default(),
                    ));
                }
            }
            write_out(&rows)
        }
        Output::Text => write_out(&format!(
            "{}: {} bytes, {} → GPT, {} ESP bytes, {} ext4 root bytes ({})\n",
            args.disk.display(),
            plan["Bytes"],
            plan["ExistingTable"].as_str().unwrap_or_default(),
            plan["Partitions"][0]["Bytes"],
            plan["Partitions"][1]["Bytes"],
            if args.apply { "applied" } else { "plan" },
        )),
    }?;
    Ok(0)
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
    build::run_in_store(&settings, &built, &args.program, &args.arguments)
        .map_err(|error| (version::failure_status(), error))?;
    Ok(0)
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
    if matches!(
        &cli.command,
        Commands::Package {
            command: PackageCommands::BuildPackage(_),
        } | Commands::System {
            command: SystemCommands::BuildSystem(_)
                | SystemCommands::BuildImage(_)
                | SystemCommands::ExportSystem(_),
        }
    ) {
        unsafe { libc::umask(0o022) };
    }
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err((code, error)) => {
            eprintln!("{error}");
            std::process::exit(code);
        }
    }
}
