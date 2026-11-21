//! Building a package into the store (`docs/build.md`).

use std::collections::BTreeSet;
use std::fs;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use vug::steps::{self, Bind, Sandbox, Step};
use vug::{fetch, rewrite, treehash};

use crate::naming::model;
use crate::record::{self, BuildOptions};
use crate::version;

const STORE: &str = "/vug/store";
const PLACEHOLDER_ID: &str = "00000000000000000000000000000000";
const SOURCE_DATE_EPOCH: &str = "315532800";

pub struct Settings {
    /// Repository root containing `seed.toml` and `packages/`.
    pub root: PathBuf,
    /// Directory holding `store/`, `index/`, `seed/`, and `tmp/`.
    pub data_dir: PathBuf,
    /// Directory holding `sources/`.
    pub cache_dir: PathBuf,
    pub jobs: u32,
}

pub struct Built {
    pub name: String,
    pub version: String,
    pub id: String,
    /// Whether this call ran the build (false when the index had the output).
    pub fresh: bool,
    /// Ids of the output and of everything it needs at run time.
    closure: BTreeSet<String>,
}

fn io<T>(what: &str, path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{what} {}: {e}", path.display()))
}

fn fetch_source(settings: &Settings, url: &str, sha256: &str) -> Result<PathBuf, String> {
    fetch::fetch(url, sha256, &settings.cache_dir.join("sources")).map_err(|e| match e {
        fetch::FetchError::Http(message) => format!("fetch {url}: {message}"),
        fetch::FetchError::HashMismatch { expected, actual } => {
            format!("fetch {url}: sha256 is {actual}, the record says {expected}")
        }
        fetch::FetchError::Io(e) => format!("fetch {url}: {e}"),
    })
}

fn run_sandbox(sandbox: &Sandbox) -> Result<(), String> {
    let stderr = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|e| format!("stderr: {e}"))?;
    let status = Command::new("bwrap")
        .args(steps::bwrap_args(sandbox))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stderr))
        .status()
        .map_err(|e| format!("bwrap: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("build failed: {status}"))
    }
}

fn build_env(path: String) -> Vec<(String, String)> {
    [
        ("PATH", path.as_str()),
        ("HOME", "/build"),
        ("SOURCE_DATE_EPOCH", SOURCE_DATE_EPOCH),
        ("ZIG_GLOBAL_CACHE_DIR", "/tmp/zig-global"),
        ("ZIG_LOCAL_CACHE_DIR", "/tmp/zig-local"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
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
fn ensure_seed(settings: &Settings) -> Result<(PathBuf, String), String> {
    let seed_path = settings.root.join("seed.toml");
    let text = io("read", &seed_path, fs::read_to_string(&seed_path))?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| format!("seed.toml: {e}"))?;
    let hash: [u8; 20] = blake3::hash(text.as_bytes()).as_bytes()[..20]
        .try_into()
        .map_err(|_| "seed hash".to_owned())?;
    let id = treehash::encode_base32(&hash);
    let seed_dir = settings.data_dir.join("seed").join(&id);
    if seed_dir.exists() {
        return Ok((seed_dir, id));
    }

    let busybox = fetch_source(
        settings,
        &seed_field(&table, "busybox", "url")?,
        &seed_field(&table, "busybox", "sha256")?,
    )?;
    let zig = fetch_source(
        settings,
        &seed_field(&table, "zig", "url")?,
        &seed_field(&table, "zig", "sha256")?,
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

    for (name, tool) in [("cc", "cc"), ("c++", "c++")] {
        write_executable(
            &bin.join(name),
            &format!(
                "#!/bin/sh\nfor a; do\n  shift\n  case \"$a\" in\n    --target=*) ;;\n    *) set -- \"$@\" \"$a\" ;;\n  esac\ndone\nexec /seed/zig/zig {tool} -target x86_64-linux-musl \"$@\"\n"
            ),
        )?;
    }
    for (name, tool) in [("ar", "ar"), ("ranlib", "ranlib"), ("ld", "ld.lld")] {
        write_executable(
            &bin.join(name),
            &format!("#!/bin/sh\nexec /seed/zig/zig {tool} \"$@\"\n"),
        )?;
    }
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
        env: build_env("/bin".to_owned()),
        workdir: "/seed/zig".to_owned(),
        command: ["tar", "-xf", "/src/zig.tar.xz", "--strip-components=1"]
            .map(str::to_owned)
            .to_vec(),
    })?;
    io("rename", &seed_dir, fs::rename(&staging, &seed_dir))?;
    Ok((seed_dir, id))
}

fn version_stems(versions_dir: &Path) -> Result<Vec<String>, String> {
    let mut stems = Vec::new();
    for entry in io("read", versions_dir, fs::read_dir(versions_dir))? {
        let entry = io("read", versions_dir, entry)?;
        if let Some(stem) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.strip_suffix(".toml"))
        {
            stems.push(stem.to_owned());
        }
    }
    Ok(stems)
}

fn current_version(versions_dir: &Path) -> Result<String, String> {
    version_stems(versions_dir)?
        .into_iter()
        .reduce(|best, v| {
            if version::compare(&v, &best) == version::model::Ordering::Gt {
                v
            } else {
                best
            }
        })
        .ok_or_else(|| format!("{}: no version records", versions_dir.display()))
}

fn file_name_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

pub struct Builder<'a> {
    settings: &'a Settings,
    seed_dir: PathBuf,
    seed_id: String,
    in_progress: Vec<String>,
}

impl<'a> Builder<'a> {
    pub fn new(settings: &'a Settings) -> Result<Self, String> {
        let (seed_dir, seed_id) = ensure_seed(settings)?;
        Ok(Self {
            settings,
            seed_dir,
            seed_id,
            in_progress: Vec::new(),
        })
    }

    pub fn build(&mut self, name: &str, requested: Option<&str>) -> Result<Built, String> {
        if !model::valid_name(name) {
            return Err(format!("{name:?}: not a valid package name"));
        }
        if self.in_progress.iter().any(|n| n == name) {
            return Err(format!(
                "dependency cycle: {} -> {name}",
                self.in_progress.join(" -> ")
            ));
        }
        self.in_progress.push(name.to_owned());
        let result = self.build_inner(name, requested);
        self.in_progress.pop();
        result
    }

    fn build_inner(&mut self, name: &str, requested: Option<&str>) -> Result<Built, String> {
        let settings = self.settings;
        let package_dir = settings.root.join(model::package_dir(name));
        let version = match requested {
            Some(v) => v.to_owned(),
            None => current_version(&package_dir.join("versions"))?,
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

        let mut dep_ids = BTreeSet::new();
        let mut run_closure = BTreeSet::new();
        let mut dep_names = record.deps_build.clone();
        dep_names.extend(record.deps_run.iter().cloned());
        dep_names.sort();
        dep_names.dedup();
        for dep in &dep_names {
            let built = self.build(dep, None)?;
            dep_ids.extend(built.closure.iter().cloned());
            if record.deps_run.contains(dep) {
                run_closure.extend(built.closure.iter().cloned());
            }
        }

        let mut hasher = blake3::Hasher::new();
        hasher.update(canonical.as_bytes());
        hasher.update(self.seed_id.as_bytes());
        for id in &dep_ids {
            hasher.update(id.as_bytes());
        }
        let key_bytes: [u8; 20] = hasher.finalize().as_bytes()[..20]
            .try_into()
            .map_err(|_| "key hash".to_owned())?;
        let key = treehash::encode_base32(&key_bytes);

        let store_dir = settings.data_dir.join("store");
        let index_path = settings.data_dir.join("index").join(name).join(&version);
        if let Ok(line) = fs::read_to_string(&index_path)
            && let Some((found_key, id)) = line.trim_end().split_once(' ')
            && found_key == key
            && store_dir.join(id).exists()
        {
            run_closure.insert(id.to_owned());
            return Ok(Built {
                name: name.to_owned(),
                version,
                id: id.to_owned(),
                fresh: false,
                closure: run_closure,
            });
        }

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
        io(
            "create",
            &build_dir,
            fs::create_dir_all(build_dir.join("src")),
        )?;

        let prefix = format!("{STORE}/{PLACEHOLDER_ID}");
        let archive_dest = format!("/src/{}", file_name_of(&record.url));
        let patches: Vec<String> = record
            .patches
            .iter()
            .map(|p| format!("/patches/{p}"))
            .collect();
        let strip = u32::try_from(record.strip_components)
            .map_err(|_| "source.strip-components: out of range".to_owned())?;
        let mut script: Vec<Step> = steps::unpack_steps(&archive_dest, strip, &patches);
        script.extend(match &record.build {
            BuildOptions::Autotools {
                configure_flags,
                make_flags,
                install_flags,
            } => steps::autotools_steps(
                &prefix,
                &steps::Autotools {
                    configure_flags: configure_flags.clone(),
                    make_flags: make_flags.clone(),
                    install_flags: install_flags.clone(),
                    check: record.check_enabled,
                    jobs: settings.jobs,
                },
            ),
            BuildOptions::BootstrapMake { configure_flags } => {
                steps::bootstrap_make_steps(&prefix, configure_flags)
            }
            BuildOptions::Prebuilt { install } => steps::prebuilt_steps(&prefix, install),
        });
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
        let mut binds = vec![
            bind(&self.seed_dir, "/seed", false),
            bind(&self.seed_dir.join("bin"), "/bin", false),
            bind(&build_dir, "/build", true),
            bind(&archive, &archive_dest, false),
            bind(&out, &prefix, true),
        ];
        if !record.patches.is_empty() {
            binds.push(bind(&package_dir.join("patches"), "/patches", false));
        }
        let mut path = String::new();
        for id in &dep_ids {
            binds.push(bind(&store_dir.join(id), &format!("{STORE}/{id}"), false));
            path.push_str(&format!("{STORE}/{id}/bin:"));
        }
        path.push_str("/bin");

        eprintln!("building {name} {version}");
        run_sandbox(&Sandbox {
            binds,
            env: build_env(path),
            workdir: "/build/src".to_owned(),
            command: ["sh", "/build/build.sh"].map(str::to_owned).to_vec(),
        })?;

        let meta = out.join(".vug");
        io("create", &meta, fs::create_dir_all(&meta))?;
        io(
            "write",
            &meta,
            fs::write(meta.join("record.toml"), &canonical),
        )?;
        let id = treehash::encode_base32(&io("hash", &out, treehash::hash_tree(&out))?);
        io(
            "rewrite",
            &out,
            rewrite::rewrite_tree(&out, prefix.as_bytes(), format!("{STORE}/{id}").as_bytes()),
        )?;
        io("create", &store_dir, fs::create_dir_all(&store_dir))?;
        let final_path = store_dir.join(&id);
        if !final_path.exists() {
            io("rename", &final_path, fs::rename(&out, &final_path))?;
        }
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
        })
    }
}
