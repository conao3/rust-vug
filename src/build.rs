//! Building a package into the store (`docs/build.md`).

use std::collections::{BTreeMap, BTreeSet};
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

/// Part of every index key. Increase it whenever the steps, flags, or
/// environment that a build receives change, so that outputs recorded under
/// the previous rules are built again.
const BUILD_RULES: &str = "7";

/// `bin/cc` and `bin/c++` of the seed. `{tool}` is `cc` or `c++`.
///
/// The wrapper adds `VUG_CFLAGS`, and `VUG_LDFLAGS` unless the call only
/// compiles or preprocesses, and turns the GNU and clang spellings that
/// packages use into what zig accepts (`docs/build.md`, "The seed").
const COMPILER_WRAPPER: &str = r#"#!/bin/sh
link=1
asm=0
dep=0
case " $* " in
  *" -fsyntax-only "*) syntax_only=1 ;;
esac
case " $* " in
  *" -nostdinc "*) nostdinc=1 ;;
esac
for a; do
  shift
  is_output=0
  if [ "${want_output:-0}" = 1 ]; then
    want_output=0
    output=$a
    is_output=1
  fi
  if [ "${want_rpath:-0}" = 1 ]; then
    want_rpath=0
    set -- "$@" "-Wl,-rpath,${a#-Wl,}"
    continue
  fi
  if [ "${want_lang:-0}" = 1 ]; then
    want_lang=0
    if [ "${syntax_only:-0}" = 1 ]; then
      case "$a" in
        c-header) a=c ;;
        c++-header) a=c++ ;;
      esac
    fi
    set -- "$@" "$a"
    continue
  fi
  if [ "${isystem:-0}" = 1 ]; then
    isystem=0
    set -- "$@" "-I$a"
    continue
  fi
  case "$a" in
    --target=* | -mtune=generic) ;;
    -isystem) isystem=1 ;;
    -isystem?*) set -- "$@" "-I${a#-isystem}" ;;
    -m16) set -- "$@" -m16 -Xclang -target-feature -Xclang +16bit-mode ;;
    -mno-sse) set -- "$@" -mno-sse -mno-sse2 -mno-sse3 -mno-ssse3 -mno-sse4.1 -mno-sse4.2 -mno-avx -mno-avx2 ;;
    -march=x86-64) set -- "$@" -march=x86_64 ;;
    -march=x86-64-v[234]) set -- "$@" "-march=x86_64_v${a#-march=x86-64-v}" ;;
    -S) link=0; asm=1; set -- "$@" "$a" ;;
    -E) link=0; preprocess=1; set -- "$@" "$a" ;;
    -c | -M | -MM) link=0; set -- "$@" "$a" ;;
    -MD | -MMD | -Wp,-MD,* | -Wp,-MMD,*) dep=1; set -- "$@" "$a" ;;
    -Wl,*)
      words=
      last=
      IFS=,
      for w in ${a#-Wl,}; do
        if [ "${skip_word:-0}" = 1 ]; then
          skip_word=0
          continue
        fi
        if [ -n "${take:-}" ]; then
          eval "${take}_fn=\$w"
          take=
          continue
        fi
        last=$w
        case "$w" in
          -init | --init) take=init ;;
          -fini | --fini) take=fini ;;
          -init=* | --init=*) init_fn=${w#*=} ;;
          -fini=* | --fini=*) fini_fn=${w#*=} ;;
          -stats | -lc | -shared | --allow-multiple-definition | -Map=* | --Map=*) ;;
          -Map | --Map) skip_word=1 ;;
          -h) words="$words,-soname" ;;
          -h?*) words="$words,-soname,${w#-h}" ;;
          -R) words="$words,-rpath" ;;
          -R?*) words="$words,-rpath,${w#-R}" ;;
          *) words="$words,$w" ;;
        esac
      done
      unset IFS
      case "$last" in
        -rpath | --rpath | -R)
          words=${words%,-rpath}
          want_rpath=1
          ;;
      esac
      if [ -n "$words" ]; then set -- "$@" "-Wl$words"; fi
      ;;
    -o) output_given=1; want_output=1; set -- "$@" "$a" ;;
    -) stdin=1; set -- "$@" "$a" ;;
    -x) lang=1; want_lang=1; set -- "$@" "$a" ;;
    -x?*) lang=1; set -- "$@" "$a" ;;
    -nostdlib) nostdlib=1; set -- "$@" "$a" ;;
    -fsyntax-only) link=0 ;;
    -*) set -- "$@" "$a" ;;
    *.o | *.lo | *.a | *.c | *.cc | *.cpp | *.cxx | *.C | *.s | *.S) objects_given=1; set -- "$@" "$a" ;;
    *.h | *.hh | *.hpp | *.i | *.ii | *.m | *.ld | *.lds | *.map | *.sym | *.def | *.txt | *.d) set -- "$@" "$a" ;;
    *)
      if [ "$is_output" = 0 ] && [ -f "$a" ] && [ "$(dd if="$a" bs=1 count=4 2>/dev/null | od -An -c | tr -d ' ')" = '177ELF' ] && [ "$(dd if="$a" bs=1 skip=16 count=1 2>/dev/null | od -An -tu1 | tr -d ' ')" = 1 ]; then
        objects=/tmp/vug-cc-$$
        mkdir -p "$objects"
        n=$((${n:-0} + 1))
        ln -s "$(realpath "$a")" "$objects/$n.o"
        objects_given=1
        set -- "$@" "$objects/$n.o"
      else
        set -- "$@" "$a"
      fi
      ;;
  esac
done
if [ "${syntax_only:-0}" = 1 ]; then
  mkdir -p /tmp/vug-cc-$$
  set -- "$@" -c -o /tmp/vug-cc-$$/syntax.o
fi
if [ "${nostdlib:-0}" = 1 ] && [ "${nostdinc:-0}" = 0 ]; then
  for d in x86_64-linux-musl generic-musl x86-linux-any any-linux-any; do
    set -- "$@" -isystem "/seed/zig/lib/libc/include/$d"
  done
fi
if [ "$link" = 1 ] && [ -n "${init_fn:-}${fini_fn:-}" ]; then
  mkdir -p /tmp/vug-cc-$$
  {
    if [ -n "${init_fn:-}" ]; then
      printf 'extern void %s(void);\n__attribute__((constructor)) static void vug_init(void) { %s(); }\n' "$init_fn" "$init_fn"
    fi
    if [ -n "${fini_fn:-}" ]; then
      printf 'extern void %s(void);\n__attribute__((destructor)) static void vug_fini(void) { %s(); }\n' "$fini_fn" "$fini_fn"
    fi
  } > /tmp/vug-cc-$$/init.c
  objects_given=1
  set -- "$@" /tmp/vug-cc-$$/init.c
fi
set -- "$@" -Wno-error=unused-command-line-argument -Wno-error=date-time
if [ "$link" = 1 ] && [ "${objects_given:-0}" = 0 ] && [ "${stdin:-0}" = 0 ] && [ "${output_given:-0}" = 1 ]; then
  mkdir -p /tmp/vug-cc-$$
  : > /tmp/vug-cc-$$/empty.c
  set -- "$@" /tmp/vug-cc-$$/empty.c
fi
if [ "${stdin:-0}" = 1 ] && [ "${lang:-0}" = 0 ]; then
  set -- -x {lang} "$@"
fi
if [ "$asm" = 1 ] && [ "$dep" = 1 ]; then
  (
    out=
    prev=
    for a; do
      shift
      if [ "$prev" = -o ]; then out=$a; set -- "$@" /dev/null
      elif [ "$a" = -S ]; then set -- "$@" -E
      else set -- "$@" "$a"; fi
      prev=$a
    done
    exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} -MT "$out" > /dev/null
  ) || exit
  for a; do
    shift
    case "$a" in
      -MD | -MMD | -Wp,-MD,* | -Wp,-MMD,*) ;;
      *) set -- "$@" "$a" ;;
    esac
  done
fi
if [ "${stdin:-0}" = 1 ] && [ "${preprocess:-0}" = 1 ]; then
  marker='s|"[^"]*zig-local/tmp/[0-9a-f]*-stdin\.c"|"<stdin>"|'
  if [ -n "${output:-}" ]; then
    /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} || exit
    if [ -f "$output" ]; then sed -i "$marker" "$output"; fi
    exit 0
  fi
  /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} > /tmp/vug-cc-$$.i
  status=$?
  sed "$marker" /tmp/vug-cc-$$.i
  rm -f /tmp/vug-cc-$$.i
  exit $status
fi
if [ "$link" = 1 ]; then
  exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} ${VUG_LDFLAGS-}
fi
exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-}
"#;

/// `bin/ar`, `bin/ranlib`, and `bin/ld` of the seed.
/// `bin/cpp` of the seed: the C preprocessor, reading standard input when
/// no input file is named.
const CPP_WRAPPER: &str = r#"#!/bin/sh
for a; do
  case "$a" in
    -) exec /seed/bin/cc -E "$@" ;;
    -*) ;;
    *) exec /seed/bin/cc -E "$@" ;;
  esac
done
exec /seed/bin/cc -E "$@" -
"#;

const TOOL_WRAPPER: &str = "#!/bin/sh\nexec /seed/zig/zig {tool} \"$@\"\n";

pub struct Settings {
    /// Repository root containing `seed.toml` and `packages/`.
    pub root: PathBuf,
    /// Directory holding `store/`, `index/`, `seed/`, and `tmp/`.
    pub data_dir: PathBuf,
    /// Directory holding `sources/`.
    pub cache_dir: PathBuf,
    pub jobs: u32,
    /// Base URL of a binary cache (`docs/cache.md`).
    pub binary_cache: Option<String>,
    /// Public key that signs the key-to-id answers of the binary cache.
    pub trusted_key: Option<String>,
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

/// Moves the tree `out`, built for the placeholder path, into the store:
/// hashes it, replaces the placeholder by the final path, and renames it.
/// Returns the id.
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
        })
    }

    /// Builds `name` (or takes it from the index or the binary cache) and
    /// returns it with its output and run-time closure present in the store.
    pub fn build(&mut self, name: &str, requested: Option<&str>) -> Result<Built, String> {
        self.resolve(name, requested, true)
    }

    /// Finds the id of `name`. With `present` the output and its run-time
    /// closure are also put into the store; without it an id that the index
    /// or the binary cache knows is returned as it is.
    fn resolve(
        &mut self,
        name: &str,
        requested: Option<&str>,
        present: bool,
    ) -> Result<Built, String> {
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
        let result = self.build_inner(name, requested, present);
        self.in_progress.pop();
        result
    }

    /// The id a binary cache records for the build `key`.
    fn cached_key(&self, key: &str) -> Result<Option<String>, String> {
        let (Some(base), Some(trusted)) = (&self.settings.binary_cache, &self.settings.trusted_key)
        else {
            return Ok(None);
        };
        let url = format!("{}/keys/{key}", base.trim_end_matches('/'));
        let Some(text) = fetch::get(&url)
            .map_err(|e| format!("fetch {url}: {e}"))?
            .and_then(|bytes| String::from_utf8(bytes).ok())
        else {
            return Ok(None);
        };
        let mut lines = text.lines();
        let id = lines.next().unwrap_or_default();
        let signature = lines.next().unwrap_or_default();
        if !vug::sign::verify(trusted, &format!("{key} {id}"), signature) {
            return Err(format!("{url}: not signed by the trusted key"));
        }
        Ok(Some(id.to_owned())
            .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())))
    }

    /// Takes the store path `id` from the binary cache. Returns whether the
    /// cache had it. The unpacked tree must hash to `id`.
    fn substitute(&self, id: &str) -> Result<bool, String> {
        let settings = self.settings;
        let Some(base) = &settings.binary_cache else {
            return Ok(false);
        };
        let url = format!("{}/paths/{id}.cpio", base.trim_end_matches('/'));
        let Some(archive) = fetch::get(&url).map_err(|e| format!("fetch {url}: {e}"))? else {
            return Ok(false);
        };
        eprintln!("substituting {STORE}/{id}");
        let work = settings
            .data_dir
            .join("tmp")
            .join(format!("{}-substitute", std::process::id()));
        if work.exists() {
            io("remove", &work, fs::remove_dir_all(&work))?;
        }
        let out = work.join(id);
        io("create", &out, fs::create_dir_all(&out))?;
        io("unpack", &out, vug::cpio::read_newc(&archive, &out))?;
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
        io("remove", &work, fs::remove_dir_all(&work))?;
        Ok(true)
    }

    fn build_inner(
        &mut self,
        name: &str,
        requested: Option<&str>,
        present: bool,
    ) -> Result<Built, String> {
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
        let mut direct = BTreeMap::new();
        let mut run_closure = BTreeSet::new();
        let mut dep_names = record.deps_build.clone();
        dep_names.extend(record.deps_run.iter().cloned());
        dep_names.sort();
        dep_names.dedup();
        for dep in &dep_names {
            let built = self.resolve(dep, None, present && record.deps_run.contains(dep))?;
            dep_ids.extend(built.closure.iter().cloned());
            direct.insert(dep.clone(), format!("{STORE}/{}", built.id));
            if record.deps_run.contains(dep) {
                run_closure.extend(built.closure.iter().cloned());
            }
        }

        let mut hasher = blake3::Hasher::new();
        hasher.update(canonical.as_bytes());
        hasher.update(self.seed_id.as_bytes());
        hasher.update(BUILD_RULES.as_bytes());
        for id in &dep_ids {
            hasher.update(id.as_bytes());
        }
        let key_bytes: [u8; 20] = hasher.finalize().as_bytes()[..20]
            .try_into()
            .map_err(|_| "key hash".to_owned())?;
        let key = treehash::encode_base32(&key_bytes);

        let store_dir = settings.data_dir.join("store");
        let index_path = settings.data_dir.join("index").join(name).join(&version);
        let mut known = fs::read_to_string(&index_path).ok().and_then(|line| {
            let (found_key, id) = line.trim_end().split_once(' ')?;
            (found_key == key).then(|| id.to_owned())
        });
        if known.is_none()
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
            && (!present || store_dir.join(&id).exists() || self.substitute(&id)?)
        {
            run_closure.insert(id.clone());
            return Ok(Built {
                name: name.to_owned(),
                version,
                id,
                fresh: false,
                closure: run_closure,
            });
        }

        for dep in &dep_names {
            self.resolve(dep, None, true)?;
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
        let archive_dest = format!("/src/{}", file_name_of(&record.url));
        let patches: Vec<String> = record
            .patches
            .iter()
            .map(|p| format!("/patches/{p}"))
            .collect();
        let strip = u32::try_from(record.strip_components)
            .map_err(|_| "source.strip-components: out of range".to_owned())?;
        let mut cflags = Vec::new();
        let mut ldflags = vec![
            "-Wl,--undefined-version".to_owned(),
            format!("-Wl,-rpath,{prefix}/lib"),
        ];
        let mut build_rpaths = Vec::new();
        let mut pkg_config_path = Vec::new();
        let mut pkg_config = String::new();
        let mut python_path = Vec::new();
        let mut aclocal_path = Vec::new();
        for id in &dep_ids {
            let host = store_dir.join(id);
            let inside = format!("{STORE}/{id}");
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
                if run_closure.contains(id) {
                    ldflags.push(format!("-Wl,-rpath,{inside}/lib"));
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
        let mut script: Vec<Step> = steps::unpack_steps(&archive_dest, strip, &patches);
        script.extend(match &record.build {
            BuildOptions::Autotools {
                configure_flags,
                make_flags,
                install_flags,
                install_targets,
                configure_command,
            } => steps::autotools_steps(
                &prefix,
                &steps::Autotools {
                    configure_command: expand(configure_command)?,
                    configure_flags: expand(configure_flags)?,
                    make_flags: expand(make_flags)?,
                    install_flags: expand(install_flags)?,
                    install_targets: install_targets.clone(),
                    check: record.check_enabled,
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
                record.check_enabled,
                settings.jobs,
            ),
            BuildOptions::Cmake { setup_flags } => steps::cmake_steps(
                &prefix,
                &expand(setup_flags)?,
                record.check_enabled,
                settings.jobs,
            ),
            BuildOptions::Meson { setup_flags } => steps::meson_steps(
                &prefix,
                &expand(setup_flags)?,
                record.check_enabled,
                settings.jobs,
            ),
            BuildOptions::Linux { defconfig, config } => {
                let mut host_ldflags = ldflags.clone();
                host_ldflags.extend(build_rpaths.iter().cloned());
                steps::linux_steps(
                    &prefix,
                    defconfig,
                    config,
                    settings.jobs,
                    &cflags.join(" "),
                    &host_ldflags.join(" "),
                )
            }
            BuildOptions::Prebuilt { install } => steps::prebuilt_steps(&prefix, install),
        });
        for (path, target) in &record.links {
            let link = format!("{prefix}/{path}");
            if let Some((parent, _)) = link.rsplit_once('/') {
                script.push(vec!["mkdir".to_owned(), "-p".to_owned(), parent.to_owned()]);
            }
            script.push(vec!["ln".to_owned(), "-s".to_owned(), target.clone(), link]);
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
        let mut binds = vec![
            bind(&seed_dir, "/seed", false),
            bind(&root_bin, "/bin", false),
            bind(&root_bin, "/usr/bin", false),
            bind(&build_dir, "/build", true),
            bind(&archive, &archive_dest, false),
            bind(&out, &prefix, true),
        ];
        if !record.patches.is_empty() {
            binds.push(bind(&package_dir.join("patches"), "/patches", false));
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
            binds,
            env: build_env(path, &extra),
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
        })
    }
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

/// Writes the binary cache of the data directory into `directory`
/// (`docs/cache.md`). Returns the numbers of keys and of archives written.
pub fn export_cache(
    data_dir: &Path,
    directory: &Path,
    secret: Option<&str>,
) -> Result<(usize, usize), String> {
    let keys_dir = directory.join("keys");
    let paths_dir = directory.join("paths");
    io("create", &keys_dir, fs::create_dir_all(&keys_dir))?;
    io("create", &paths_dir, fs::create_dir_all(&paths_dir))?;
    let index = data_dir.join("index");
    let (mut keys, mut paths) = (0, 0);
    let list = |dir: &Path| -> Result<Vec<PathBuf>, String> {
        let mut entries: Vec<PathBuf> = io("read", dir, fs::read_dir(dir))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        Ok(entries)
    };
    for package in list(&index)? {
        for version in list(&package)? {
            let line = io("read", &version, fs::read_to_string(&version))?;
            let Some((key, id)) = line.trim_end().split_once(' ') else {
                continue;
            };
            let key_path = keys_dir.join(key);
            let signature = match secret {
                Some(secret) => format!("{}\n", vug::sign::sign(secret, &format!("{key} {id}"))?),
                None => String::new(),
            };
            io(
                "write",
                &key_path,
                fs::write(&key_path, format!("{id}\n{signature}")),
            )?;
            keys += 1;
            let tree = data_dir.join("store").join(id);
            let archive = paths_dir.join(format!("{id}.cpio"));
            if tree.exists() && !archive.exists() {
                let partial = paths_dir.join(format!("{id}.cpio.partial"));
                let mut out =
                    std::io::BufWriter::new(io("create", &partial, fs::File::create(&partial))?);
                io("write", &partial, vug::cpio::write_newc(&tree, &mut out))?;
                drop(out);
                io("rename", &archive, fs::rename(&partial, &archive))?;
                paths += 1;
            }
        }
    }
    Ok((keys, paths))
}
