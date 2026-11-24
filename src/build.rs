//! Building a package into the store (`docs/build.md`).

mod cache;
pub(crate) mod elf;
mod go;
mod node;
mod outputs;
mod source_zip;

pub use cache::export_cache;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use vug::steps::{self, Bind, Sandbox, Step};
use vug::wrapper::{COMPILER_WRAPPER, CPP_WRAPPER, TOOL_WRAPPER};
use vug::{fetch, rewrite, treehash};

use crate::naming::model;
use crate::record::{self, BuildOptions};

const STORE: &str = "/vug/store";
const PLACEHOLDER_ID: &str = "00000000000000000000000000000000";
const SOURCE_DATE_EPOCH: &str = "315532800";

/// Part of every index key. Increase it whenever the steps, flags, or
/// environment that a build receives change, so that outputs recorded under
/// the previous rules are built again.
const BUILD_RULES: &str = "9";

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct OutputBinding<'a> {
    role: String,
    package: &'a str,
    output: &'a str,
    id: &'a str,
}

fn build_key(
    canonical: &str,
    seed_id: &str,
    dep_ids: &BTreeSet<String>,
    bindings: Option<&BTreeSet<OutputBinding<'_>>>,
) -> String {
    let mut hasher = if bindings.is_some() {
        blake3::Hasher::new_derive_key("vug build key for selected outputs v1")
    } else {
        blake3::Hasher::new()
    };
    if let Some(bindings) = bindings {
        let mut field = |value: &str| {
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        };
        for value in [canonical, seed_id, BUILD_RULES] {
            field(value);
        }
        field(&dep_ids.len().to_string());
        for id in dep_ids {
            field(id);
        }
        for binding in bindings {
            for value in [
                binding.role.as_str(),
                binding.package,
                binding.output,
                binding.id,
            ] {
                field(value);
            }
        }
    } else {
        hasher.update(canonical.as_bytes());
        hasher.update(seed_id.as_bytes());
        hasher.update(BUILD_RULES.as_bytes());
        for id in dep_ids {
            hasher.update(id.as_bytes());
        }
    }
    treehash::encode_base32(hasher.finalize().as_bytes().first_chunk().unwrap())
}

pub struct Settings {
    /// Repository root containing `seed.toml` and `packages/`.
    pub root: PathBuf,
    /// Directory holding `store/`, `index/`, `seed/`, and `tmp/`.
    pub data_dir: PathBuf,
    /// Directory holding `sources/`.
    pub cache_dir: PathBuf,
    pub source_cache_dir: Option<PathBuf>,
    pub jobs: u32,
    /// Base URL of a binary cache (`docs/cache.md`).
    pub binary_cache: Option<String>,
    /// Public key that signs the key-to-id answers of the binary cache.
    pub trusted_key: Option<String>,
}

#[derive(Clone)]
pub struct Built {
    pub name: String,
    pub version: String,
    pub id: String,
    /// Whether this call ran the build (false when the index had the output).
    pub fresh: bool,
    /// Ids of the output and of everything it needs at run time.
    closure: BTreeSet<String>,
    /// Ids that a build against this output sees as well: what it and its
    /// run-time dependencies expose.
    exposed: BTreeSet<String>,
}

fn io<T>(what: &str, path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{what} {}: {e}", path.display()))
}

fn verified_source(cache: &Path, sha256: &str) -> Result<PathBuf, String> {
    if !crate::merge::is_hex(sha256, 64) {
        return Err("expected a lowercase SHA-256 source digest".into());
    }
    let path = cache.join(sha256);
    let actual = io("verify offline source", &path, fetch::sha256_file(&path))?;
    if actual != sha256 {
        return Err(format!(
            "offline source {}: expected {sha256}, got {actual}",
            path.display()
        ));
    }
    Ok(path)
}

fn fetch_source(settings: &Settings, url: &str, sha256: &str) -> Result<PathBuf, String> {
    if let Some(cache) = &settings.source_cache_dir {
        return verified_source(cache, sha256);
    }
    fetch::fetch(url, sha256, &settings.cache_dir.join("sources")).map_err(|e| match e {
        fetch::FetchError::Http(message) => format!("fetch {url}: {message}"),
        fetch::FetchError::HashMismatch { expected, actual } => {
            format!("fetch {url}: sha256 is {actual}, the record says {expected}")
        }
        fetch::FetchError::Io(e) => format!("fetch {url}: {e}"),
    })
}

fn offline_patch(cache: &Path, patch: &Path) -> Result<PathBuf, String> {
    let digest = io("hash patch", patch, fetch::sha256_file(patch))?;
    verified_source(cache, &digest)
}

fn run_sandbox(sandbox: &Sandbox) -> Result<(), String> {
    let stderr = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|e| format!("stderr: {e}"))?;
    let mut command = Command::new("bwrap");
    command
        .args(steps::bwrap_args(sandbox))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stderr));
    // A build must not see which signals the caller of vug ignores (`nohup`
    // leaves SIGHUP ignored, and an ignored signal stays ignored across exec).
    // SAFETY: `signal` is async-signal-safe and the closure touches no memory
    // of the parent.
    unsafe {
        command.pre_exec(|| {
            for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
                libc::signal(signal, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let status = command.status().map_err(|e| format!("bwrap: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("build failed: {status}"))
    }
}

fn build_env(path: String, extra: &[(&str, String)]) -> Vec<(String, String)> {
    [
        ("PATH", path.as_str()),
        ("HOME", "/build"),
        ("SOURCE_DATE_EPOCH", SOURCE_DATE_EPOCH),
        ("build_alias", "x86_64-unknown-linux-musl"),
        ("ZIG_GLOBAL_CACHE_DIR", "/tmp/zig-global"),
        ("ZIG_LOCAL_CACHE_DIR", "/tmp/zig-local"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .chain(
        extra
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| ((*k).to_owned(), v.clone())),
    )
    .collect()
}

fn write_executable(path: &Path, text: &str) -> Result<(), String> {
    if path.symlink_metadata().is_ok() {
        io("remove", path, fs::remove_file(path))?;
    }
    io("write", path, fs::write(path, text))?;
    io(
        "chmod",
        path,
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)),
    )
}

fn seed_field(table: &toml::Table, tool: &str, key: &str) -> Result<String, String> {
    table
        .get(tool)
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("seed.toml: {tool}.{key}: expected a string"))
}

/// Assembles the seed tools described by `<root>/seed.toml` and returns the
/// host directory that is bound to `/seed`.
/// Fetches the crates that `Cargo.lock` in `build_dir` names into
/// `build_dir/crates/<name>-<version>.crate`, each checked against the
/// checksum of the lock file. Returns (name, version, sha256) per crate.
fn fetch_crates(
    settings: &Settings,
    build_dir: &Path,
) -> Result<Vec<(String, String, String)>, String> {
    const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";
    let lock_path = build_dir.join("Cargo.lock");
    let lock = io("read", &lock_path, fs::read_to_string(&lock_path))?
        .parse::<toml::Table>()
        .map_err(|e| format!("Cargo.lock: {e}"))?;
    let crates_dir = build_dir.join("crates");
    io("create", &crates_dir, fs::create_dir_all(&crates_dir))?;
    let mut crates = Vec::new();
    let packages = lock.get("package").and_then(|v| v.as_array());
    for package in packages.into_iter().flatten() {
        let field = |key: &str| package.get(key).and_then(|v| v.as_str());
        let (Some(name), Some(version)) = (field("name"), field("version")) else {
            return Err("Cargo.lock: a package without name or version".to_owned());
        };
        let Some(source) = field("source") else {
            continue;
        };
        let (true, Some(sha256)) = (source == REGISTRY, field("checksum")) else {
            return Err(format!(
                "Cargo.lock: {name} {version} comes from {source}; only crates.io with a checksum is supported"
            ));
        };
        let url = format!("https://static.crates.io/crates/{name}/{name}-{version}.crate");
        let file = fetch_source(settings, &url, sha256)?;
        let dest = crates_dir.join(format!("{name}-{version}.crate"));
        io("copy", &dest, fs::copy(&file, &dest))?;
        crates.push((name.to_owned(), version.to_owned(), sha256.to_owned()));
    }
    Ok(crates)
}

/// Reads `seed.toml` and returns it with the id of the seed.
fn read_seed(settings: &Settings) -> Result<(toml::Table, String), String> {
    let seed_path = settings.root.join("seed.toml");
    let text = io("read", &seed_path, fs::read_to_string(&seed_path))?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| format!("seed.toml: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(text.as_bytes());
    hasher.update(COMPILER_WRAPPER.as_bytes());
    hasher.update(TOOL_WRAPPER.as_bytes());
    hasher.update(CPP_WRAPPER.as_bytes());
    let hash: [u8; 20] = hasher.finalize().as_bytes()[..20]
        .try_into()
        .map_err(|_| "seed hash".to_owned())?;
    Ok((table, treehash::encode_base32(&hash)))
}

fn fetch_busybox(settings: &Settings, table: &toml::Table) -> Result<PathBuf, String> {
    fetch_source(
        settings,
        &seed_field(table, "busybox", "url")?,
        &seed_field(table, "busybox", "sha256")?,
    )
}

/// Assembles the seed `id` in the data directory when it is missing.
fn ensure_seed(settings: &Settings, table: &toml::Table, id: &str) -> Result<PathBuf, String> {
    let seed_dir = settings.data_dir.join("seed").join(id);
    if seed_dir.exists() {
        return Ok(seed_dir);
    }

    let busybox = fetch_busybox(settings, table)?;
    let zig = fetch_source(
        settings,
        &seed_field(table, "zig", "url")?,
        &seed_field(table, "zig", "sha256")?,
    )?;

    let staging = settings.data_dir.join("seed").join(format!("{id}.tmp"));
    if staging.exists() {
        io("remove", &staging, fs::remove_dir_all(&staging))?;
    }
    let bin = staging.join("bin");
    io("create", &bin, fs::create_dir_all(&bin))?;
    io("create", &staging, fs::create_dir_all(staging.join("zig")))?;
    let busybox_bin = bin.join("busybox");
    io("copy", &busybox_bin, fs::copy(&busybox, &busybox_bin))?;
    io(
        "chmod",
        &busybox_bin,
        fs::set_permissions(&busybox_bin, fs::Permissions::from_mode(0o755)),
    )?;

    let listing = Command::new(&busybox_bin)
        .arg("--list")
        .output()
        .map_err(|e| format!("busybox --list: {e}"))?;
    if !listing.status.success() {
        return Err(format!("busybox --list: {}", listing.status));
    }
    for applet in String::from_utf8_lossy(&listing.stdout).lines() {
        if applet != "busybox" && !applet.is_empty() {
            let link = bin.join(applet);
            io(
                "symlink",
                &link,
                std::os::unix::fs::symlink("busybox", &link),
            )?;
        }
    }

    for tool in ["cc", "c++"] {
        write_executable(
            &bin.join(tool),
            &COMPILER_WRAPPER
                .replace("{tool}", tool)
                .replace("{lang}", if tool == "cc" { "c" } else { "c++" }),
        )?;
    }
    for (name, tool) in [("ar", "ar"), ("ranlib", "ranlib"), ("ld", "ld.lld")] {
        write_executable(&bin.join(name), &TOOL_WRAPPER.replace("{tool}", tool))?;
    }
    let cpp = bin.join("cpp");
    if cpp.symlink_metadata().is_ok() {
        io("remove", &cpp, fs::remove_file(&cpp))?;
    }
    write_executable(&cpp, CPP_WRAPPER)?;
    let gcc = bin.join("gcc");
    io("symlink", &gcc, std::os::unix::fs::symlink("cc", &gcc))?;

    run_sandbox(&Sandbox {
        binds: vec![
            Bind {
                source: staging.display().to_string(),
                dest: "/seed".to_owned(),
                writable: true,
            },
            Bind {
                source: bin.display().to_string(),
                dest: "/bin".to_owned(),
                writable: false,
            },
            Bind {
                source: zig.display().to_string(),
                dest: "/src/zig.tar.xz".to_owned(),
                writable: false,
            },
        ],
        env: build_env("/bin".to_owned(), &[]),
        workdir: "/seed/zig".to_owned(),
        command: ["tar", "-xf", "/src/zig.tar.xz", "--strip-components=1"]
            .map(str::to_owned)
            .to_vec(),
    })?;
    io("rename", &seed_dir, fs::rename(&staging, &seed_dir))?;
    Ok(seed_dir)
}

/// Puts a link `into/<name>` to `<inside>/<name>` for every entry of
/// `host_dir`, replacing a link of the same name. A missing `host_dir` adds
/// nothing.
fn link_programs(host_dir: &Path, inside: &str, into: &Path) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(host_dir) else {
        return Ok(());
    };
    let mut names: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.file_name()))
        .collect();
    names.sort();
    for name in names {
        let link = into.join(&name);
        if link.symlink_metadata().is_ok() {
            io("remove", &link, fs::remove_file(&link))?;
        }
        let target = Path::new(inside).join(&name);
        io("symlink", &link, std::os::unix::fs::symlink(&target, &link))?;
    }
    Ok(())
}

/// The file name of the musl dynamic loader in `lib`, when there is one.
fn loader_name(lib: &Path) -> Option<String> {
    let mut names: Vec<String> = fs::read_dir(lib)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("ld-musl-") && name.ends_with(".so.1"))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// Replaces `{out}` by the output path and `{dep:NAME}` by the store path of
/// the dependency NAME.
fn expand_flag(
    flag: &str,
    prefix: &str,
    direct: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = flag;
    while let Some(start) = rest.find("{dep:") {
        let (before, tail) = rest.split_at(start);
        let Some(end) = tail.find('}') else {
            break;
        };
        let name = &tail["{dep:".len()..end];
        let path = direct
            .get(name)
            .ok_or_else(|| format!("{flag:?}: {name:?} is not a dependency of this package"))?;
        out.push_str(before);
        out.push_str(path);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out.replace("{out}", prefix))
}

fn file_name_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

fn prepare_archive(archive: PathBuf, name: &str, work: &Path) -> Result<(PathBuf, String), String> {
    if name.ends_with(".tar.lz") {
        let tar = work.join("source.tar");
        let input = std::io::BufReader::new(io("open", &archive, fs::File::open(&archive))?);
        let mut decoder = lzma_rust2::LzipReader::new(input);
        let mut output = io("create", &tar, fs::File::create(&tar))?;
        io(
            "decompress lzip",
            &archive,
            std::io::copy(&mut decoder, &mut output),
        )?;
        Ok((tar, format!("/src/{}", name.trim_end_matches(".lz"))))
    } else {
        Ok((archive, format!("/src/{name}")))
    }
}

/// Moves the tree `out`, built for the placeholder path, into the store:
/// hashes it, replaces the placeholder by the final path, and renames it.
/// Returns the id.
/// The first file under `dir` that holds a statically linked libc++
/// (`docs/build.md`, "The C++ runtime"), found by a message of libc++abi
/// and one of libc++.
fn private_cxx_runtime(dir: &Path) -> Result<Option<PathBuf>, String> {
    // The messages are written backwards so that this program does not hold
    // the bytes it searches for.
    const REVERSED: [&[u8]; 2] = [
        b"denruter yldetcepxenu reldnah_etanimret",
        b"rorre yrogetac_cireneg deificepsnu",
    ];
    let mut entries: Vec<PathBuf> = io("read", dir, fs::read_dir(dir))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    entries.sort();
    for path in entries {
        let meta = io("stat", &path, fs::symlink_metadata(&path))?;
        if meta.is_dir() {
            if let Some(found) = private_cxx_runtime(&path)? {
                return Ok(Some(found));
            }
        } else if meta.is_file() {
            let bytes = io("read", &path, fs::read(&path))?;
            if bytes.starts_with(b"\x7fELF")
                && REVERSED.iter().any(|marker| {
                    bytes
                        .windows(marker.len())
                        .any(|window| window.iter().rev().eq(marker.iter()))
                })
            {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

pub fn install_output(store_dir: &Path, out: &Path) -> Result<String, String> {
    let id = treehash::encode_base32(&io("hash", out, treehash::hash_tree(out))?);
    io(
        "rewrite",
        out,
        rewrite::rewrite_tree(
            out,
            placeholder_path().as_bytes(),
            format!("{STORE}/{id}").as_bytes(),
        ),
    )?;
    io("create", store_dir, fs::create_dir_all(store_dir))?;
    let final_path = store_dir.join(&id);
    if !final_path.exists()
        && let Err(error) = fs::rename(out, &final_path)
        && !final_path.exists()
    {
        return Err(format!("rename {}: {error}", final_path.display()));
    }
    Ok(id)
}

/// The path an output is built for before its id is known.
pub fn placeholder_path() -> String {
    format!("{STORE}/{PLACEHOLDER_ID}")
}

impl Built {
    /// Ids of the output and of everything it needs at run time.
    pub fn closure(&self) -> &BTreeSet<String> {
        &self.closure
    }
}

pub struct Builder<'a> {
    settings: &'a Settings,
    seed: toml::Table,
    seed_id: String,
    in_progress: Vec<String>,
    cache_only: bool,
    cache_inventory: Option<CacheInventory>,
    build_from_source: BTreeSet<String>,
}

#[derive(Default)]
struct CacheInventory {
    objects: BTreeSet<String>,
    missing: bool,
    resolved: BTreeMap<(String, Option<String>), Option<Built>>,
}

pub fn list_cache(settings: &Settings) -> Result<BTreeSet<String>, String> {
    let mut builder = Builder::cached(settings)?;
    builder.cache_inventory = Some(CacheInventory::default());
    let packages = settings.root.join("packages");
    for shard in io("read", &packages, fs::read_dir(&packages))? {
        let shard = io("read entry", &packages, shard)?.path();
        if !shard.is_dir() {
            continue;
        }
        for entry in io("read", &shard, fs::read_dir(&shard))? {
            let package = io("read entry", &shard, entry)?.path();
            let Some(name) = package.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !model::valid_name(name) || !package.join("package.toml").is_file() {
                continue;
            }
            let inventory = builder.cache_inventory.as_mut().unwrap();
            inventory.missing = false;
            if let Err(error) = builder.resolve(name, None, Need::Id)
                && !builder.cache_inventory.as_ref().unwrap().missing
            {
                return Err(error);
            }
        }
    }
    Ok(builder.cache_inventory.take().unwrap().objects)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Need {
    Id,
    Runtime,
    Build,
}

impl<'a> Builder<'a> {
    /// The busybox of the seed, as a file on the host: the one of the
    /// running system when it is the same file, else the fetched one.
    pub fn busybox(&self) -> Result<PathBuf, String> {
        let running = PathBuf::from("/run/current-system/bin/busybox");
        if let Ok(actual) = fetch::sha256_file(&running)
            && actual == seed_field(&self.seed, "busybox", "sha256")?
        {
            return Ok(running);
        }
        fetch_busybox(self.settings, &self.seed)
    }

    pub fn new(settings: &'a Settings) -> Result<Self, String> {
        let (seed, seed_id) = read_seed(settings)?;
        Ok(Self {
            settings,
            seed,
            seed_id,
            in_progress: Vec::new(),
            cache_only: false,
            cache_inventory: None,
            build_from_source: BTreeSet::new(),
        })
    }

    pub fn cached(settings: &'a Settings) -> Result<Self, String> {
        let mut builder = Self::new(settings)?;
        builder.cache_only = true;
        Ok(builder)
    }

    pub fn build_from_source(&mut self, names: &[String]) -> Result<(), String> {
        for name in names {
            if !model::valid_name(name) {
                return Err(format!("{name:?}: invalid package name"));
            }
            self.build_from_source.insert(name.clone());
        }
        Ok(())
    }

    pub fn require_output(&self, id: &str) -> Result<(), String> {
        if self.ensure_output(id)? {
            return Ok(());
        }
        Err(format!(
            "{STORE}/{id}: output missing from the store and binary cache"
        ))
    }

    fn ensure_output(&self, id: &str) -> Result<bool, String> {
        let path = self.settings.data_dir.join("store").join(id);
        let metadata = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !self.substitute(id)? {
                    return Ok(false);
                }
                fs::symlink_metadata(&path)
            }
            result => result,
        };
        if !io("read store reference", &path, metadata)?.is_dir() {
            return Err(format!("{}: expected a store directory", path.display()));
        }
        Ok(true)
    }

    /// Builds `name` (or takes it from the index or the binary cache) and
    /// returns it with its output and run-time closure present in the store.
    pub fn build(&mut self, name: &str, requested: Option<&str>) -> Result<Built, String> {
        let (package, output) = record::dependency_output(name);
        if !model::valid_name(package) || !model::valid_name(output) {
            return Err(format!("{name:?}: not a valid package or output selector"));
        }
        if let Some(cache) = &self.settings.source_cache_dir {
            for tool in ["busybox", "zig"] {
                verified_source(cache, &seed_field(&self.seed, tool, "sha256")?)?;
            }
            self.preflight_sources(cache, package, requested, &mut BTreeSet::new())?;
        }
        self.resolve(name, requested, Need::Runtime)
    }

    fn preflight_sources(
        &self,
        cache: &Path,
        name: &str,
        requested: Option<&str>,
        visited: &mut BTreeSet<(String, String)>,
    ) -> Result<(), String> {
        let directory = self.settings.root.join(model::package_dir(name));
        let version = match requested {
            Some(version) if model::valid_version(version) => version.to_owned(),
            Some(version) => return Err(format!("{version:?}: not a valid version")),
            None => record::current_version(&directory.join("versions"))?,
        };
        if !visited.insert((name.into(), version.clone())) {
            return Ok(());
        }
        let file = self.settings.root.join(model::version_path(name, &version));
        let (record, _) = record::load_version(&file).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        verified_source(cache, &record.sha256)?;
        for patch in &record.patches {
            offline_patch(cache, &directory.join("patches").join(patch))?;
        }
        for (_, dependency) in record
            .dependencies()
            .filter(|(_, dep)| !dep.starts_with(':'))
        {
            self.preflight_sources(
                cache,
                record::dependency_output(dependency).0,
                None,
                visited,
            )?;
        }
        Ok(())
    }

    /// Resolves the selector, materializing its runtime closure or its
    /// runtime closure and exposed build dependencies as requested.
    fn resolve(
        &mut self,
        spec: &str,
        requested: Option<&str>,
        need: Need,
    ) -> Result<Built, String> {
        let (name, output) = record::dependency_output(spec);
        if self.in_progress.iter().any(|n| n == name) {
            return Err(format!(
                "dependency cycle: {} -> {name}",
                self.in_progress.join(" -> ")
            ));
        }
        let lookup = self
            .cache_inventory
            .is_some()
            .then(|| (spec.to_owned(), requested.map(str::to_owned)));
        if let Some(lookup) = &lookup
            && let Some(inventory) = &mut self.cache_inventory
            && let Some(built) = inventory.resolved.get(lookup)
        {
            return match built {
                Some(built) => Ok(built.clone()),
                None => {
                    inventory.missing = true;
                    Err(format!("{spec}: no signed cache entry"))
                }
            };
        }
        self.in_progress.push(name.to_owned());
        let result = self.build_inner(name, output, requested, need);
        self.in_progress.pop();
        if let Some(lookup) = lookup
            && let Some(inventory) = &mut self.cache_inventory
            && (result.is_ok() || inventory.missing)
        {
            inventory
                .resolved
                .insert(lookup, result.as_ref().ok().cloned());
        }
        result
    }

    /// The id a binary cache records for the build `key`.
    fn cached_key(&self, key: &str) -> Result<Option<String>, String> {
        let (Some(base), Some(trusted)) = (&self.settings.binary_cache, &self.settings.trusted_key)
        else {
            return Ok(None);
        };
        let url = format!("{}/keys/{key}", base.trim_end_matches('/'));
        let Some(bytes) = fetch::get(&url).map_err(|e| format!("fetch {url}: {e}"))? else {
            return Ok(None);
        };
        let text = std::str::from_utf8(&bytes).map_err(|e| format!("{url}: {e}"))?;
        let mut lines = text.lines();
        let id = lines.next().unwrap_or_default();
        let signature = lines.next().unwrap_or_default();
        if !vug::sign::verify(trusted, &format!("{key} {id}"), signature) {
            return Err(format!("{url}: not signed by the trusted key"));
        }
        if !valid_store_id(id) {
            return Err(format!("{url}: invalid store id {id:?}"));
        }
        Ok(Some(id.to_owned()))
    }

    /// Takes the store path `id` from the binary cache. Returns whether the
    /// cache had it. The unpacked tree must hash to `id`.
    fn substitute(&self, id: &str) -> Result<bool, String> {
        let settings = self.settings;
        let Some(base) = &settings.binary_cache else {
            return Ok(false);
        };
        let url = format!("{}/paths/{id}.cpio.zst", base.trim_end_matches('/'));
        let Some(archive) = fetch::get_reader(&url).map_err(|e| format!("fetch {url}: {e}"))?
        else {
            return Ok(false);
        };
        eprintln!("substituting {STORE}/{id}");
        let tmp = settings.data_dir.join("tmp");
        io("create", &tmp, fs::create_dir_all(&tmp))?;
        let work = io(
            "create temporary directory",
            &tmp,
            tempfile::tempdir_in(&tmp),
        )?;
        let out = work.path().join(id);
        io("create", &out, fs::create_dir(&out))?;
        let mut tree = zstd::Decoder::new(archive).map_err(|error| format!("{url}: {error}"))?;
        vug::cpio::read_newc(&mut tree, &out).map_err(|error| format!("{url}: {error}"))?;
        std::io::copy(&mut tree, &mut std::io::sink())
            .map_err(|error| format!("{url}: {error}"))?;
        let path = format!("{STORE}/{id}");
        let placeholder = placeholder_path();
        io(
            "rewrite",
            &out,
            rewrite::rewrite_tree(&out, path.as_bytes(), placeholder.as_bytes()),
        )?;
        let actual = treehash::encode_base32(&io("hash", &out, treehash::hash_tree(&out))?);
        if actual != id {
            return Err(format!("{url}: the content hashes to {actual}"));
        }
        io(
            "rewrite",
            &out,
            rewrite::rewrite_tree(&out, placeholder.as_bytes(), path.as_bytes()),
        )?;
        let store_dir = settings.data_dir.join("store");
        io("create", &store_dir, fs::create_dir_all(&store_dir))?;
        let final_path = store_dir.join(id);
        if !final_path.exists()
            && let Err(error) = fs::rename(&out, &final_path)
            && !final_path.exists()
        {
            return Err(format!("rename {}: {error}", final_path.display()));
        }
        Ok(true)
    }

    fn build_inner(
        &mut self,
        name: &str,
        output: &str,
        requested: Option<&str>,
        need: Need,
    ) -> Result<Built, String> {
        let settings = self.settings;
        let package_dir = settings.root.join(model::package_dir(name));
        let version = match requested {
            Some(v) => v.to_owned(),
            None => record::current_version(&package_dir.join("versions"))?,
        };
        if !model::valid_version(&version) {
            return Err(format!("{version:?}: not a valid version"));
        }
        let record_path = settings.root.join(model::version_path(name, &version));
        let (record, canonical) = record::load_version(&record_path).map_err(|errors| {
            errors
                .iter()
                .map(|e| format!("{}: {}", e.path, e.message))
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        if !record.has_output(output) {
            return Err(format!(
                "{name}:{output}: unknown output in version {version}"
            ));
        }
        let split = !record.outputs.is_empty();
        let selected_run = outputs::selected_run(&record, output);
        let mut dep_ids = BTreeSet::new();
        let mut direct = BTreeMap::new();
        let mut resolved = BTreeMap::new();
        let mut run_closure = BTreeSet::new();
        let mut build_run_closure = BTreeSet::new();
        let mut exposed = BTreeSet::new();
        let all_run: BTreeSet<_> = record
            .deps_run
            .iter()
            .chain(
                record
                    .outputs
                    .values()
                    .flat_map(|output| output.run.iter().filter(|dep| !dep.starts_with(':'))),
            )
            .map(String::as_str)
            .collect();
        let dep_names: BTreeSet<_> = record
            .deps_build
            .iter()
            .map(String::as_str)
            .chain(all_run.iter().copied())
            .collect();
        for &dep in &dep_names {
            let built = self.resolve(
                dep,
                None,
                if selected_run.contains(dep) {
                    need
                } else {
                    Need::Id
                },
            )?;
            dep_ids.extend(built.closure.iter().cloned());
            dep_ids.extend(built.exposed.iter().cloned());
            direct.insert(dep.to_owned(), format!("{STORE}/{}", built.id));
            if all_run.contains(dep) {
                build_run_closure.extend(built.closure.iter().cloned());
            }
            if selected_run.contains(dep) {
                run_closure.extend(built.closure.iter().cloned());
                exposed.extend(built.exposed.iter().cloned());
            }
            resolved.insert(dep, built);
        }
        for dep in &record.deps_expose {
            let built =
                self.resolve(dep, None, if need == Need::Build { need } else { Need::Id })?;
            exposed.extend(built.closure.iter().cloned());
            exposed.extend(built.exposed.iter().cloned());
            resolved.entry(dep.as_str()).or_insert(built);
        }

        let bindings = record.uses_output_keys().then(|| {
            record
                .dependencies()
                .filter_map(|(role, dep)| {
                    let (package, output) = record::dependency_output(dep);
                    (!package.is_empty()).then(|| OutputBinding {
                        role,
                        package,
                        output,
                        id: &resolved[dep].id,
                    })
                })
                .collect()
        });
        let kind_key;
        let key_record = match &record.build {
            BuildOptions::Python { .. } => {
                kind_key = format!("python-build-rules=v1\n{canonical}");
                &kind_key
            }
            BuildOptions::Node { .. } => {
                kind_key = format!("node-build-rules=v1\n{canonical}");
                &kind_key
            }
            BuildOptions::Go { .. } => {
                kind_key = format!("go-build-rules=v2\n{canonical}");
                &kind_key
            }
            BuildOptions::Perl { .. } => {
                kind_key = format!("perl-build-rules=v1\n{canonical}");
                &kind_key
            }
            _ => &canonical,
        };
        let key = build_key(key_record, &self.seed_id, &dep_ids, bindings.as_ref());

        let store_dir = settings.data_dir.join("store");
        let index_dir = if split { "index-outputs" } else { "index" };
        let index_path = settings.data_dir.join(index_dir).join(name).join(&version);
        let runtime = outputs::runtime(&record, &resolved);
        let from_manifest = |manifest: &outputs::Manifest, fresh| {
            let id = manifest.outputs[output].id.clone();
            let mut closure = run_closure.clone();
            closure.insert(id.clone());
            closure.extend(
                runtime[output]
                    .local
                    .iter()
                    .map(|local| manifest.outputs[local].id.clone()),
            );
            Built {
                name: name.to_owned(),
                version: version.clone(),
                id,
                fresh,
                closure,
                exposed: exposed.clone(),
            }
        };
        let mut expected_manifest = None;
        if split {
            let local = if self.cache_inventory.is_none() {
                outputs::Manifest::read(&index_path, &key, &runtime)?
            } else {
                None
            };
            let from_cache = local.is_none();
            let manifest = match local {
                Some(manifest) => Some(manifest),
                None if self.build_from_source.contains(name) => None,
                None => cache::fetch_manifest(settings, &key, &runtime)?,
            };
            if let Some(manifest) = manifest {
                let built = from_manifest(&manifest, false);
                if let Some(inventory) = &mut self.cache_inventory {
                    inventory.objects.insert(format!("output-keys/{key}"));
                    inventory.objects.extend(
                        manifest
                            .outputs
                            .values()
                            .map(|entry| format!("paths/{}.cpio.zst", entry.id)),
                    );
                }
                if need == Need::Id {
                    return Ok(built);
                }
                let mut missing = None;
                for name in
                    std::iter::once(output).chain(runtime[output].local.iter().map(String::as_str))
                {
                    let id = &manifest.outputs[name].id;
                    if !self.ensure_output(id)? {
                        missing = Some(id);
                        break;
                    }
                    manifest.check_tree(name, &store_dir)?;
                }
                if let Some(id) = missing {
                    if self.cache_only {
                        return Err(format!(
                            "{STORE}/{id}: output missing from the store and binary cache"
                        ));
                    }
                    expected_manifest = Some(manifest);
                } else {
                    if from_cache {
                        manifest.publish(&index_path, &runtime)?;
                    }
                    return Ok(built);
                }
            }
        } else if self.cache_inventory.is_some() {
            if let Some(id) = self.cached_key(&key)? {
                let inventory = self.cache_inventory.as_mut().unwrap();
                inventory.objects.insert(format!("keys/{key}"));
                inventory.objects.insert(format!("paths/{id}.cpio.zst"));
                run_closure.insert(id.clone());
                return Ok(Built {
                    name: name.to_owned(),
                    version,
                    id,
                    fresh: false,
                    closure: run_closure,
                    exposed,
                });
            }
        } else {
            let mut known = fs::read_to_string(&index_path).ok().and_then(|line| {
                let (found_key, id) = line.trim_end().split_once(' ')?;
                (found_key == key && valid_store_id(id)).then(|| id.to_owned())
            });
            if known.is_none()
                && !self.build_from_source.contains(name)
                && let Some(id) = self.cached_key(&key)?
            {
                if let Some(parent) = index_path.parent() {
                    io("create", parent, fs::create_dir_all(parent))?;
                }
                io(
                    "write",
                    &index_path,
                    fs::write(&index_path, format!("{key} {id}\n")),
                )?;
                known = Some(id);
            }
            if let Some(id) = known
                && (need == Need::Id || self.ensure_output(&id)?)
            {
                run_closure.insert(id.clone());
                return Ok(Built {
                    name: name.to_owned(),
                    version,
                    id,
                    fresh: false,
                    closure: run_closure,
                    exposed,
                });
            }
        }

        if self.cache_only {
            if let Some(inventory) = &mut self.cache_inventory {
                inventory.missing = true;
            }
            return Err(format!(
                "{name} {version}: no output available from the local index or signed binary cache (build key {key})"
            ));
        }

        for dep in dep_names {
            let built = self.resolve(dep, None, Need::Build)?;
            let expected = &resolved[dep];
            if built.id != expected.id
                || built.closure != expected.closure
                || built.exposed != expected.exposed
            {
                return Err(format!(
                    "{dep}: output changed while resolving dependencies"
                ));
            }
        }
        let seed_dir = ensure_seed(settings, &self.seed, &self.seed_id)?;
        let archive = fetch_source(settings, &record.url, &record.sha256)?;
        let work = settings
            .data_dir
            .join("tmp")
            .join(format!("{}-{name}", std::process::id()));
        if work.exists() {
            io("remove", &work, fs::remove_dir_all(&work))?;
        }
        let out = work.join("out");
        let build_dir = work.join("build");
        io("create", &out, fs::create_dir_all(&out))?;
        let root_bin = work.join("bin");
        io("create", &root_bin, fs::create_dir_all(&root_bin))?;
        link_programs(&seed_dir.join("bin"), "/seed/bin", &root_bin)?;
        for id in &dep_ids {
            link_programs(
                &store_dir.join(id).join("bin"),
                &format!("{STORE}/{id}/bin"),
                &root_bin,
            )?;
        }
        io(
            "create",
            &build_dir,
            fs::create_dir_all(build_dir.join("src")),
        )?;

        let prefix = format!("{STORE}/{PLACEHOLDER_ID}");
        let patches: Vec<String> = record
            .patches
            .iter()
            .map(|p| format!("/patches/{p}"))
            .collect();
        let strip = u32::try_from(record.strip_components)
            .map_err(|_| "source.strip-components: out of range".to_owned())?;
        let source_name = file_name_of(&record.url);
        let archive = if source_name.ends_with(".zip") {
            io(
                "extract ZIP",
                &archive,
                source_zip::extract(&archive, &build_dir.join("src"), strip),
            )?;
            None
        } else {
            Some(prepare_archive(archive, source_name, &work)?)
        };
        let mut cflags = Vec::new();
        let own_lib = if split { "lib/." } else { "lib" };
        let mut ldflags = vec![
            "-Wl,--undefined-version".to_owned(),
            format!("-Wl,-rpath,{prefix}/{own_lib}"),
        ];
        if split {
            ldflags.push(format!(
                "-Wl,-rpath,{}",
                outputs::rpath_space(record.outputs.len())
            ));
        }
        let mut auto_rpaths = BTreeSet::new();
        let mut build_rpaths = Vec::new();
        let mut pkg_config_path = Vec::new();
        let mut pkg_config = String::new();
        let mut python_path = Vec::new();
        let mut aclocal_path = Vec::new();
        let mut libcxx = String::new();
        for id in &dep_ids {
            let host = store_dir.join(id);
            let inside = format!("{STORE}/{id}");
            if host.join("include/c++/v1/__config_site").is_file() {
                libcxx = inside.clone();
            }
            if let Some(loader) = loader_name(&host.join("lib")) {
                cflags.push("-fPIC".to_owned());
                ldflags.push("-dynamic".to_owned());
                ldflags.push(format!("-Wl,--dynamic-linker={inside}/lib/{loader}"));
                continue;
            }
            if host.join("share/aclocal").is_dir() {
                aclocal_path.push(format!("{inside}/share/aclocal"));
            }
            if host.join("include").is_dir() {
                cflags.push("-isystem".to_owned());
                cflags.push(format!("{inside}/include"));
            }
            if host.join("lib").is_dir() {
                ldflags.push(format!("-L{inside}/lib"));
                if build_run_closure.contains(id) {
                    let rpath = if split {
                        let path = format!("{inside}/lib/.");
                        auto_rpaths.insert(path.clone());
                        path
                    } else {
                        format!("{inside}/lib")
                    };
                    ldflags.push(format!("-Wl,-rpath,{rpath}"));
                } else {
                    build_rpaths.push(format!("-Wl,-rpath,{inside}/lib"));
                }
            }
            for dir in ["lib/pkgconfig", "share/pkgconfig"] {
                if host.join(dir).is_dir() {
                    pkg_config_path.push(format!("{inside}/{dir}"));
                }
            }
            if host.join("bin/pkgconf").is_file() {
                pkg_config = format!("{inside}/bin/pkgconf");
            }
            if let Ok(entries) = fs::read_dir(host.join("lib")) {
                let mut names: Vec<String> = entries
                    .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                    .filter(|name| name.starts_with("python"))
                    .collect();
                names.sort();
                for name in names {
                    if host.join("lib").join(&name).join("site-packages").is_dir() {
                        python_path.push(format!("{inside}/lib/{name}/site-packages"));
                    }
                }
            }
        }
        let host_tools = matches!(record.build, BuildOptions::Linux { .. });
        let extra = [
            (
                "VUG_CFLAGS",
                if host_tools {
                    String::new()
                } else {
                    cflags.join(" ")
                },
            ),
            (
                "VUG_LDFLAGS",
                if host_tools {
                    String::new()
                } else {
                    ldflags.join(" ")
                },
            ),
            ("VUG_LIBCXX", libcxx),
            ("PKG_CONFIG_PATH", pkg_config_path.join(":")),
            ("PKG_CONFIG", pkg_config),
            ("PYTHONPATH", python_path.join(":")),
            ("ACLOCAL_PATH", aclocal_path.join(":")),
            (
                "CMAKE_PREFIX_PATH",
                direct.values().cloned().collect::<Vec<_>>().join(":"),
            ),
            (
                "LDFLAGS",
                if matches!(record.build, BuildOptions::Meson { .. }) {
                    ldflags
                        .iter()
                        .filter(|flag| flag.starts_with("-Wl,-rpath,"))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    String::new()
                },
            ),
        ];
        let expand = |flags: &[String]| -> Result<Vec<String>, String> {
            flags
                .iter()
                .map(|flag| expand_flag(flag, &prefix, &direct))
                .collect()
        };
        let mut script: Vec<Step> = match &archive {
            Some((_, destination)) => steps::unpack_steps(destination, strip, &patches),
            None => steps::patch_steps(&patches).collect(),
        };
        if let BuildOptions::Autotools { path, .. } | BuildOptions::Meson { path, .. } =
            &record.build
            && path != "."
        {
            script.push(vec!["cd".to_owned(), format!("./{path}")]);
        }
        let check_target = record
            .check_enabled
            .then(|| record.check_target.as_deref().unwrap_or("check"));
        script.extend(match &record.build {
            BuildOptions::Autotools {
                autoreconf,
                configure_flags,
                make_flags,
                install_flags,
                install_targets,
                configure_command,
                ..
            } => steps::autotools_steps(
                &prefix,
                &steps::Autotools {
                    autoreconf: *autoreconf,
                    configure_command: expand(configure_command)?,
                    configure_flags: expand(configure_flags)?,
                    make_flags: expand(make_flags)?,
                    install_flags: expand(install_flags)?,
                    install_targets: install_targets.clone(),
                    check_target: check_target.map(str::to_owned),
                    check_skip: record.check_skip.clone(),
                    jobs: settings.jobs,
                },
            ),
            BuildOptions::BootstrapMake { configure_flags } => {
                steps::bootstrap_make_steps(&prefix, &expand(configure_flags)?)
            }
            BuildOptions::Make {
                make_flags,
                install_flags,
                build_targets,
                install_targets,
            } => steps::make_steps(
                &expand(make_flags)?,
                &expand(install_flags)?,
                build_targets,
                install_targets,
                check_target,
                settings.jobs,
            ),
            BuildOptions::Cmake { path, setup_flags } => steps::cmake_steps(
                &prefix,
                path,
                &expand(setup_flags)?,
                record.check_enabled,
                &record.check_skip,
                settings.jobs,
            ),
            BuildOptions::Meson { setup_flags, .. } => steps::meson_steps(
                &prefix,
                &expand(setup_flags)?,
                record.check_enabled,
                &record.check_skip,
                settings.jobs,
            ),
            BuildOptions::Linux {
                defconfig,
                config,
                modules,
            } => {
                let mut host_ldflags = ldflags.clone();
                host_ldflags.extend(build_rpaths.iter().cloned());
                steps::linux_steps(
                    &prefix,
                    defconfig,
                    config,
                    settings.jobs,
                    &cflags.join(" "),
                    &host_ldflags.join(" "),
                    modules.then_some(key.as_str()),
                )
            }
            BuildOptions::Cargo { .. } => {
                vec![vec![
                    "cp".to_owned(),
                    "Cargo.lock".to_owned(),
                    "/build/Cargo.lock".to_owned(),
                ]]
            }
            BuildOptions::Node { .. } => Vec::new(),
            BuildOptions::Go { .. } => Vec::new(),
            BuildOptions::Perl { driver, modules } => {
                let perl = direct
                    .get("perl")
                    .filter(|_| record.deps_run.iter().any(|dep| dep == "perl"))
                    .ok_or("build.kind perl requires perl in dependencies.run")?;
                let mut runtime = Vec::new();
                for dep in &all_run {
                    let id = &resolved[dep].id;
                    if !runtime.contains(id) {
                        runtime.push(id.clone());
                    }
                }
                for id in &build_run_closure {
                    if !runtime.contains(id) {
                        runtime.push(id.clone());
                    }
                }
                let paths = |ids: Vec<String>| {
                    ids.iter()
                        .map(|id| format!("{STORE}/{id}"))
                        .collect::<Vec<_>>()
                        .join(":")
                };
                vec![
                    vec![
                        format!("{perl}/bin/perl"),
                        "-e".to_owned(),
                        include_str!("build/perl.pl").to_owned(),
                        "--".to_owned(),
                        prefix.clone(),
                        driver.name().to_owned(),
                        paths(dep_ids.iter().cloned().collect()),
                        paths(runtime),
                        settings.jobs.to_string(),
                        record.check_enabled.to_string(),
                        modules.join(","),
                    ]
                    .into_iter()
                    .chain(record.check_skip.iter().cloned())
                    .collect(),
                ]
            }
            BuildOptions::Python {
                imports,
                native_imports,
                pytest,
            } => {
                let python = direct
                    .get("python")
                    .filter(|_| all_run.contains("python"))
                    .ok_or("build.kind python requires python in dependencies.run")?;
                let mut runtime = Vec::new();
                for dep in &all_run {
                    let built = &resolved[dep];
                    if !runtime.contains(&built.id) {
                        runtime.push(built.id.clone());
                    }
                }
                for id in &build_run_closure {
                    if !runtime.contains(id) {
                        runtime.push(id.clone());
                    }
                }
                vec![vec![
                    "env".to_owned(),
                    "CC=cc".to_owned(),
                    "CXX=c++".to_owned(),
                    "LDSHARED=cc -shared".to_owned(),
                    format!("{python}/bin/python3"),
                    "-sB".to_owned(),
                    "-c".to_owned(),
                    include_str!("build/python.py").to_owned(),
                    prefix.clone(),
                    runtime
                        .iter()
                        .map(|id| format!("{STORE}/{id}"))
                        .collect::<Vec<_>>()
                        .join(":"),
                    if record.check_enabled {
                        imports.join(",")
                    } else {
                        String::new()
                    },
                    if record.check_enabled {
                        native_imports.join(",")
                    } else {
                        String::new()
                    },
                    (record.check_enabled && *pytest).to_string(),
                ]]
            }
            BuildOptions::Commands { commands } => commands
                .iter()
                .map(|words| expand(words))
                .collect::<Result<Vec<_>, _>>()?,
            BuildOptions::Prebuilt { install } => steps::prebuilt_steps(&prefix, install),
        });
        for (path, target) in &record.links {
            let link = format!("{prefix}/{path}");
            if let Some((parent, _)) = link.rsplit_once('/') {
                script.push(vec!["mkdir".to_owned(), "-p".to_owned(), parent.to_owned()]);
            }
            script.push(vec!["ln".to_owned(), "-s".to_owned(), target.clone(), link]);
        }
        for (path, words) in &record.wrappers {
            let file = format!("{prefix}/{path}");
            if let Some((parent, _)) = file.rsplit_once('/') {
                script.push(vec!["mkdir".to_owned(), "-p".to_owned(), parent.to_owned()]);
            }
            let command = expand(words)?
                .iter()
                .map(|word| steps::shell_quote(word))
                .collect::<Vec<_>>()
                .join(" ");
            script.push(vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "printf '%s\\n' \"$2\" \"$3\" > \"$1\" && chmod 755 \"$1\"".to_owned(),
                "sh".to_owned(),
                file,
                "#!/bin/sh".to_owned(),
                format!("exec {command} \"$@\""),
            ]);
        }
        let script_path = build_dir.join("build.sh");
        io(
            "write",
            &script_path,
            fs::write(&script_path, steps::shell_script(&script)),
        )?;

        let bind = |source: &Path, dest: &str, writable: bool| Bind {
            source: source.display().to_string(),
            dest: dest.to_owned(),
            writable,
        };
        let hosts = work.join("hosts");
        io(
            "write",
            &hosts,
            fs::write(&hosts, "127.0.0.1 localhost\n::1 localhost\n"),
        )?;
        let mut binds = vec![
            bind(&hosts, "/etc/hosts", false),
            bind(&seed_dir, "/seed", false),
            bind(&root_bin, "/bin", false),
            bind(&root_bin, "/usr/bin", false),
            bind(&build_dir, "/build", true),
            bind(&out, &prefix, true),
        ];
        if let Some((archive, destination)) = &archive {
            binds.push(bind(archive, destination, false));
        }
        if !record.patches.is_empty() {
            let original = package_dir.join("patches");
            let directory = if let Some(cache) = &settings.source_cache_dir {
                let directory = work.join("patches");
                io("create", &directory, fs::create_dir(&directory))?;
                for patch in &record.patches {
                    let source = offline_patch(cache, &original.join(patch))?;
                    let destination = directory.join(patch);
                    io("copy", &destination, fs::copy(source, &destination))?;
                }
                directory
            } else {
                original
            };
            binds.push(bind(&directory, "/patches", false));
        }
        let mut path = String::new();
        for id in &dep_ids {
            let inside = format!("{STORE}/{id}");
            binds.push(bind(&store_dir.join(id), &inside, false));
            path.push_str(&format!("{inside}/bin:"));
        }
        path.push_str("/bin");

        eprintln!("building {name} {version}");
        run_sandbox(&Sandbox {
            binds: binds.clone(),
            env: build_env(path.clone(), &extra),
            workdir: "/build/src".to_owned(),
            command: ["sh", "/build/build.sh"].map(str::to_owned).to_vec(),
        })?;
        if let BuildOptions::Cargo {
            path: member,
            flags,
        } = &record.build
        {
            let crates = fetch_crates(settings, &build_dir)?;
            io(
                "write",
                &script_path,
                fs::write(
                    &script_path,
                    steps::shell_script(&steps::cargo_steps(
                        &prefix,
                        member,
                        &expand(flags)?,
                        if record.check_enabled {
                            &record.check_skip
                        } else {
                            &[]
                        },
                        settings.jobs,
                        &crates,
                    )),
                ),
            )?;
            run_sandbox(&Sandbox {
                binds,
                env: build_env(path, &extra),
                workdir: "/build/src".to_owned(),
                command: ["sh", "/build/build.sh"].map(str::to_owned).to_vec(),
            })?;
        } else if let BuildOptions::Go { targets } = &record.build {
            let go = direct
                .get("go")
                .filter(|_| record.deps_build.iter().any(|dep| dep == "go"))
                .ok_or("build.kind go requires go in dependencies.build")?;
            let goroot = format!("{go}/lib/go");
            let sandbox = Sandbox {
                binds,
                env: build_env(path, &extra),
                workdir: "/build/src".to_owned(),
                command: ["sh", "/build/build.sh"].map(str::to_owned).to_vec(),
            };
            let vendor = go::prepare(
                &build_dir,
                &sandbox,
                &goroot,
                targets,
                record.check_enabled,
                settings.jobs,
                settings.source_cache_dir.is_some(),
            )?;
            io(
                "write",
                &script_path,
                fs::write(
                    &script_path,
                    steps::shell_script(&steps::go_steps(
                        &prefix,
                        &goroot,
                        targets,
                        vendor,
                        record.check_enabled,
                        settings.jobs,
                    )),
                ),
            )?;
            run_sandbox(&sandbox)?;
        } else if let BuildOptions::Node {
            build_script,
            musl_executables,
        } = &record.build
        {
            let node = direct
                .get("nodejs")
                .filter(|_| record.deps_run.iter().any(|dep| dep == "nodejs"))
                .ok_or("build.kind node requires nodejs in dependencies.run")?;
            let npm = direct
                .get("npm")
                .filter(|_| record.deps_build.iter().any(|dep| dep == "npm"))
                .ok_or("build.kind node requires npm in dependencies.build")?;
            let mut relocations = Vec::new();
            if !musl_executables.is_empty() {
                let musl = direct
                    .get("musl")
                    .filter(|_| record.deps_run.iter().any(|dep| dep == "musl"))
                    .ok_or("build.musl-executables requires musl in dependencies.run")?;
                let patchelf = direct
                    .get("patchelf")
                    .filter(|_| record.deps_build.iter().any(|dep| dep == "patchelf"))
                    .ok_or("build.musl-executables requires patchelf in dependencies.build")?;
                for executable in musl_executables {
                    relocations.push(vec![
                        format!("{patchelf}/bin/patchelf"),
                        "--set-interpreter".to_owned(),
                        format!("{musl}/lib/ld-musl-x86_64.so.1"),
                        format!("./{executable}"),
                    ]);
                }
            }
            let archives = node::fetch_locked(
                &build_dir,
                &settings.cache_dir,
                settings.source_cache_dir.is_some(),
            )?;
            io(
                "write",
                &script_path,
                fs::write(
                    &script_path,
                    steps::shell_script(&node::steps(
                        &prefix,
                        node,
                        npm,
                        &archives,
                        &relocations,
                        *build_script,
                        record.check_enabled,
                    )),
                ),
            )?;
            run_sandbox(&Sandbox {
                binds,
                env: build_env(path, &extra),
                workdir: "/build/src".to_owned(),
                command: ["sh", "/build/build.sh"].map(str::to_owned).to_vec(),
            })?;
        }

        let mut entries = io("read", &out, fs::read_dir(&out))?;
        let has_payload = entries.try_fold(false, |found, entry| {
            let entry = io("read", &out, entry)?;
            Ok::<_, String>(found || entry.file_name() != ".vug")
        })?;
        if !has_payload && !out.join(".vug/root").is_dir() {
            return Err(format!("{name} {version}: build installed no files"));
        }
        if !record.private_cxx_runtime
            && let Some(file) = private_cxx_runtime(&out)?
        {
            return Err(format!(
                "{name}: {} carries its own copy of the C++ runtime; add `libcxx` to dependencies.run",
                file.strip_prefix(&out).unwrap_or(&file).display()
            ));
        }
        if split {
            let manifest = outputs::SplitBuild {
                staging: &out,
                work: &work,
                store: &store_dir,
                record: &record,
                canonical: &canonical,
                runtime: &runtime,
                auto_rpaths: &auto_rpaths,
            }
            .install(&key)?;
            io("remove", &work, fs::remove_dir_all(&work))?;
            if expected_manifest.is_some_and(|expected| expected != manifest) {
                return Err(format!(
                    "{name} {version}: rebuilt outputs disagree with the manifest for build key {key}"
                ));
            }
            manifest.publish(&index_path, &runtime)?;
            return Ok(from_manifest(&manifest, true));
        }
        let meta = out.join(".vug");
        io("create", &meta, fs::create_dir_all(&meta))?;
        io(
            "write",
            &meta,
            fs::write(meta.join("record.toml"), &canonical),
        )?;
        let id = install_output(&store_dir, &out)?;
        io("remove", &work, fs::remove_dir_all(&work))?;

        if let Some(parent) = index_path.parent() {
            io("create", parent, fs::create_dir_all(parent))?;
        }
        io(
            "write",
            &index_path,
            fs::write(&index_path, format!("{key} {id}\n")),
        )?;
        run_closure.insert(id.clone());
        Ok(Built {
            name: name.to_owned(),
            version,
            id,
            fresh: true,
            closure: run_closure,
            exposed,
        })
    }
}

pub fn valid_store_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// A `bwrap` command line up to the program: a mount namespace that shows
/// the host file system and the store at `/vug/store`. With `as_root` the
/// caller appears as user 0, so every file it owns appears owned by root.
pub fn store_command(settings: &Settings, as_root: bool) -> Result<Command, String> {
    let mut command = Command::new("bwrap");
    if as_root {
        command.args(["--unshare-user", "--uid", "0", "--gid", "0"]);
    }
    let mut names: Vec<_> = io("read", Path::new("/"), fs::read_dir("/"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    names.sort();
    for path in names {
        let flag = if path == Path::new("/dev") {
            "--dev-bind"
        } else {
            "--bind"
        };
        command.arg(flag).arg(&path).arg(&path);
    }
    command
        .arg("--ro-bind")
        .arg(settings.data_dir.join("store"))
        .arg(STORE)
        .arg("--");
    Ok(command)
}

/// Replaces this process by `program` of the output `id`, in a mount
/// namespace that shows the host file system and the store at `/vug/store`.
/// Returns only when that fails, with the reason.
pub fn run_in_store(settings: &Settings, id: &str, program: &str, arguments: &[String]) -> String {
    use std::os::unix::process::CommandExt;

    let mut command = match store_command(settings, false) {
        Ok(command) => command,
        Err(error) => return error,
    };
    command
        .arg(format!("{STORE}/{id}/{program}"))
        .args(arguments);
    format!("bwrap: {}", command.exec())
}

#[cfg(test)]
mod tests {
    use super::{BUILD_RULES, OutputBinding, build_key, prepare_archive, private_cxx_runtime};
    use super::{Builder, Settings, fs, model, record};
    use std::collections::BTreeSet;
    use vug::treehash;

    #[test]
    fn legacy_key_uses_the_original_concatenation() {
        let ids = BTreeSet::from(["id-one".to_owned(), "id-two".to_owned()]);
        let hash = blake3::hash(format!("recordseed{BUILD_RULES}id-oneid-two").as_bytes());
        let expected = treehash::encode_base32(hash.as_bytes().first_chunk().unwrap());
        assert_eq!(build_key("record", "seed", &ids, None), expected);
    }

    #[test]
    fn versioned_key_commits_to_the_selected_outputs_and_roles() {
        let ids = BTreeSet::from(["id-one".to_owned(), "id-two".to_owned()]);
        let key = |role: &str, output: &str, id: &str| {
            build_key(
                "record",
                "seed",
                &ids,
                Some(&BTreeSet::from([OutputBinding {
                    role: role.to_owned(),
                    package: "llvm",
                    output,
                    id,
                }])),
            )
        };
        let original = key("dependencies.build", "dev", "id-one");
        assert_ne!(original, build_key("record", "seed", &ids, None));
        assert_ne!(original, key("dependencies.run", "dev", "id-one"));
        assert_ne!(original, key("dependencies.build", "lib", "id-one"));
        assert_ne!(original, key("dependencies.build", "dev", "id-two"));
    }

    #[test]
    fn explicit_out_selector_uses_the_versioned_key_for_an_index_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings {
            root: directory.path().to_owned(),
            data_dir: directory.path().join("data"),
            cache_dir: directory.path().join("cache"),
            source_cache_dir: None,
            jobs: 1,
            binary_cache: None,
            trusted_key: None,
        };
        fs::write(settings.root.join("seed.toml"), "").unwrap();
        let mut builder = Builder::new(&settings).unwrap();
        let write_record = |name: &str, run: &str| {
            let path = settings.root.join(model::version_path(name, "1"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path.parent().unwrap().join("../package.toml"),
                format!("name = \"{name}\"\nlicense = \"MIT\"\n[upstream]\nsources = [\"https://example.org/\"]\n"),
            )
            .unwrap();
            fs::write(
                &path,
                format!(
                    "version = \"1\"\n[source]\nurl = \"https://example.org/{name}.tar.gz\"\nsha256 = \"{}\"\n[build]\nkind = \"autotools\"\n[dependencies]\nbuild = []\nrun = [{run}]\n[check]\nenabled = false\nskip = []\n",
                    "0".repeat(64)
                ),
            )
            .unwrap();
            record::load_version(&path).ok().unwrap().1
        };
        let write_index = |name: &str, key: &str, id: &str| {
            let index = settings.data_dir.join("index").join(name);
            fs::create_dir_all(&index).unwrap();
            fs::write(index.join("1"), format!("{key} {id}\n")).unwrap();
            fs::create_dir_all(settings.data_dir.join("store").join(id)).unwrap();
        };
        let dependency_id = "a".repeat(32);
        let consumer_id = "b".repeat(32);
        let canonical = write_record("library", "");
        let key = build_key(&canonical, &builder.seed_id, &BTreeSet::new(), None);
        write_index("library", &key, &dependency_id);

        let canonical = write_record("consumer", "\"library:out\"");
        let key = build_key(
            &canonical,
            &builder.seed_id,
            &BTreeSet::from([dependency_id.clone()]),
            Some(&BTreeSet::from([OutputBinding {
                role: "dependencies.run".to_owned(),
                package: "library",
                output: "out",
                id: &dependency_id,
            }])),
        );
        write_index("consumer", &key, &consumer_id);
        let built = builder.build("consumer", None).ok().unwrap();
        assert!(!built.fresh);
        assert_eq!(built.id, consumer_id);
        assert!(built.closure().contains(&dependency_id));

        write_record("consumer", "\"library:dev\"");
        let error = builder.build("consumer", None).err().unwrap();
        assert!(error.contains("library:dev: unknown output"), "{error}");
    }

    #[test]
    fn lzip_source_decodes_to_tar_and_checks_the_trailer() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("cached");
        let mut lzip = [
            0x4c, 0x5a, 0x49, 0x50, 0x01, 0x0c, 0x00, 0x39, 0x19, 0x49, 0xda, 0xe9, 0x39, 0xa3,
            0xed, 0x32, 0x89, 0x36, 0x78, 0xe7, 0xf1, 0x5c, 0xa9, 0x15, 0xf6, 0xa3, 0x2e, 0x70,
            0xa4, 0xa9, 0x2c, 0xd4, 0x2c, 0x43, 0x4a, 0xda, 0x35, 0x57, 0x02, 0x08, 0x08, 0x7e,
            0x2d, 0x5b, 0x0f, 0x5c, 0x95, 0x2e, 0xb7, 0x9c, 0x35, 0x14, 0xa5, 0xa5, 0xc7, 0x9c,
            0xff, 0x56, 0x5a, 0xec, 0xc4, 0x56, 0xab, 0x7a, 0xab, 0xa7, 0x31, 0x1c, 0xf2, 0x6b,
            0x75, 0xba, 0x50, 0xb9, 0x5e, 0x5d, 0x22, 0xe2, 0xa6, 0xdc, 0x71, 0xac, 0x60, 0xf4,
            0x95, 0x04, 0x91, 0x98, 0xd1, 0x85, 0xc4, 0x00, 0xe2, 0x69, 0xda, 0x31, 0x6f, 0x91,
            0x7c, 0xab, 0x08, 0xd0, 0x05, 0x08, 0xff, 0xff, 0xf6, 0xb9, 0x1c, 0x20, 0x54, 0x61,
            0x8c, 0x93, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x82, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        std::fs::write(&archive, lzip).unwrap();
        let (tar, destination) =
            prepare_archive(archive.clone(), "release.tar.lz", directory.path()).unwrap();
        assert_eq!(destination, "/src/release.tar");
        assert_eq!(std::fs::read(&archive).unwrap(), lzip);
        let extraction = std::process::Command::new("tar")
            .args(["-xf"])
            .arg(tar)
            .arg("--strip-components=1")
            .current_dir(directory.path())
            .status()
            .unwrap();
        assert!(extraction.success());
        assert_eq!(
            std::fs::read(directory.path().join("hello")).unwrap(),
            b"hello\n"
        );

        let crc_offset = lzip.len() - 20;
        lzip[crc_offset] ^= 1;
        std::fs::write(&archive, lzip).unwrap();
        let error = prepare_archive(archive, "release.tar.lz", directory.path()).unwrap_err();
        assert!(error.starts_with("decompress lzip "), "{error}");
    }

    #[test]
    fn static_cxx_runtime_is_found_in_elf_files_only() {
        let message: String = "denruter yldetcepxenu reldnah_etanimret"
            .chars()
            .rev()
            .collect();
        let directory = tempfile::tempdir().expect("temporary directory");
        let lib = directory.path().join("lib");
        std::fs::create_dir(&lib).expect("create");
        std::fs::write(lib.join("notes.txt"), &message).expect("write");
        std::fs::write(lib.join("libc.so"), b"\x7fELF plain").expect("write");
        assert_eq!(private_cxx_runtime(directory.path()), Ok(None));
        let object = [b"\x7fELF ".as_slice(), message.as_bytes()].concat();
        std::fs::write(lib.join("libx.so"), object).expect("write");
        assert_eq!(
            private_cxx_runtime(directory.path()),
            Ok(Some(lib.join("libx.so")))
        );
    }
}
