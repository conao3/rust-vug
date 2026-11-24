//! Build steps and the sandbox command line. Every function here is pure:
//! it only builds lists of strings.

/// A build step: a program and its arguments.
pub type Step = Vec<String>;

const INSTALL_STAGE: &str = "/build/dest";

pub struct Autotools {
    pub autoreconf: bool,
    /// The words that start the configure step; empty means
    /// `./configure --prefix=<prefix>`.
    pub configure_command: Vec<String>,
    pub configure_flags: Vec<String>,
    pub make_flags: Vec<String>,
    pub install_flags: Vec<String>,
    /// Targets of the install step; empty means `install`.
    pub install_targets: Vec<String>,
    /// Target of the check step, if enabled.
    pub check_target: Option<String>,
    pub check_skip: Vec<String>,
    /// Parallel jobs passed as `-j<jobs>`.
    pub jobs: u32,
}

#[derive(Clone)]
pub struct Bind {
    pub source: String,
    pub dest: String,
    pub writable: bool,
}

pub struct Sandbox {
    pub binds: Vec<Bind>,
    /// Environment variables, in order.
    pub env: Vec<(String, String)>,
    pub workdir: String,
    pub command: Vec<String>,
}

/// Steps that unpack `archive` into the current directory and apply patches.
///
/// 1. `["tar", "-xf", archive, "--strip-components=<strip>"]`, or, when the
///    file name has no `.tar` in it and does not end in `.tgz`, `.txz`, or
///    `.tbz2`, `["cp", archive, <file name>]`
/// 2. for each patch, in order: validate and apply the unified diff at strip level 1.
pub fn unpack_steps(archive: &str, strip: u32, patches: &[String]) -> Vec<Step> {
    let name = archive.rsplit('/').next().unwrap_or(archive);
    let is_archive = name.contains(".tar")
        || [".tgz", ".txz", ".tbz2"]
            .iter()
            .any(|suffix| name.ends_with(suffix));
    let mut steps = vec![if is_archive {
        vec![
            "tar".to_string(),
            "-xf".to_string(),
            archive.to_string(),
            format!("--strip-components={strip}"),
        ]
    } else {
        vec!["cp".to_string(), archive.to_string(), name.to_string()]
    }];
    steps.extend(patch_steps(patches));
    steps
}

pub fn patch_steps(patches: &[String]) -> impl Iterator<Item = Step> {
    patches.iter().map(|patch| {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            include_str!("build/patch.sh").to_owned(),
            "sh".to_string(),
            patch.clone(),
        ]
    })
}

pub fn go_command(goroot: &str, jobs: u32, proxy: &str) -> Step {
    let mut step = [
        "env",
        "GOENV=off",
        "GOFLAGS=-modcacherw",
        "GOSUMDB=off",
        "GOTOOLCHAIN=local",
        "GOWORK=off",
        "CGO_ENABLED=0",
        "GOCACHE=/build/go-cache",
        "GOMODCACHE=/build/go-modcache",
    ]
    .map(str::to_owned)
    .to_vec();
    step.extend([
        format!("GOPROXY={proxy}"),
        format!("GOROOT={goroot}"),
        format!("GOMAXPROCS={jobs}"),
        format!("{goroot}/bin/go"),
    ]);
    step
}

pub fn go_steps(
    prefix: &str,
    goroot: &str,
    targets: &[(String, String)],
    vendor: bool,
    check: bool,
    jobs: u32,
) -> Vec<Step> {
    let mut steps = vec![vec![
        "mkdir".to_owned(),
        "-p".to_owned(),
        format!("{prefix}/bin"),
    ]];
    let go = |command: &str| {
        let mut step = go_command(goroot, jobs, "off");
        step.extend([
            command.to_owned(),
            if vendor {
                "-mod=vendor"
            } else {
                "-mod=readonly"
            }
            .to_owned(),
            "-trimpath".to_owned(),
            "-buildvcs=false".to_owned(),
            format!("-p={jobs}"),
        ]);
        step
    };
    for (path, bin) in targets {
        let mut step = go("build");
        let output = format!("{prefix}/bin/{bin}");
        step.extend(["-o".to_owned(), output.clone(), path.clone()]);
        steps.push(step);
        steps.push(vec!["test".to_owned(), "-x".to_owned(), output]);
    }
    if check {
        let mut step = go("test");
        step.push("./...".to_owned());
        steps.push(step);
    }
    steps
}

/// Steps of the `autotools` build kind.
///
/// 1. only when `autoreconf`: `["autoreconf", "-fi"]`
/// 2. `["./configure", "--prefix=<prefix>", <configure_flags>...]`
/// 3. `["make", "-j<jobs>", <make_flags>...]`
/// 4. only when `check`: `["make", "-j<jobs>", "check", <make_flags>...]`
/// 5. `["make", <install_targets>..., <make_flags>..., <install_flags>...,
///    "DESTDIR=/build/dest"]`
/// 6. Copy the staged prefix to the output and the remaining files to
///    `<prefix>/.vug/root`.
pub fn autotools_steps(prefix: &str, options: &Autotools) -> Vec<Step> {
    let mut configure = if options.configure_command.is_empty() {
        vec!["./configure".to_string(), format!("--prefix={prefix}")]
    } else {
        options.configure_command.clone()
    };
    configure.extend(options.configure_flags.iter().cloned());

    let jobs = format!("-j{}", options.jobs);
    let mut make = vec!["make".to_string(), jobs.clone()];
    make.extend(options.make_flags.iter().cloned());

    let mut steps = Vec::new();
    if options.autoreconf {
        steps.push(vec!["autoreconf".to_owned(), "-fi".to_owned()]);
    }
    steps.extend([configure, make]);

    if let Some(target) = &options.check_target {
        let mut check = vec!["make".to_string(), jobs, target.to_owned()];
        check.extend(options.make_flags.iter().cloned());
        if !options.check_skip.is_empty() {
            check.push(format!("VUG_CHECK_SKIP={}", options.check_skip.join(" ")));
            check.splice(..0, ["sh", "-ec", AUTOMAKE_CHECK, "sh"].map(str::to_owned));
        }
        steps.push(check);
    }

    let mut install = vec!["make".to_string()];
    if options.install_targets.is_empty() {
        install.push("install".to_string());
    } else {
        install.extend(options.install_targets.iter().cloned());
    }
    install.extend(options.make_flags.iter().cloned());
    install.extend(options.install_flags.iter().cloned());
    install.push(format!("DESTDIR={INSTALL_STAGE}"));
    steps.push(["mkdir", "-p", INSTALL_STAGE].map(str::to_owned).to_vec());
    steps.push(install);
    steps.push(collect_staged_install(prefix));

    steps
}

const AUTOMAKE_CHECK: &str = r#"
edited=$(find . -type f -name Makefile -exec sh -ec '
    for file do
        if grep -q "^# Makefile.in generated by automake " "$file" &&
           grep -q "^TESTS[[:blank:]]*=" "$file"; then
            sed -i -e "1i\\
TESTS = \$(filter-out \$(VUG_CHECK_SKIP),\$(vug_original_tests))\\
check_PROGRAMS = \$(filter-out \$(VUG_CHECK_SKIP),\$(vug_original_check_programs))" \
                -e "s/^TESTS[[:blank:]]*=/vug_original_tests =/" \
                -e "s/^check_PROGRAMS[[:blank:]]*=/vug_original_check_programs =/" "$file"
            printf "%s\n" "$file"
        fi
    done
' sh {} +)
if [ -z "$edited" ]; then
    echo "check.skip requires an Automake TESTS harness" >&2
    exit 1
fi
exec "$@"
"#;

/// Steps of the `bootstrap-make` build kind, which builds GNU make where no
/// `make` exists yet.
///
/// 1. `["./configure", "--prefix=<prefix>", <configure_flags>...]`
/// 2. `["./build.sh"]`
/// 3. `["./make", "install"]`
pub fn bootstrap_make_steps(prefix: &str, configure_flags: &[String]) -> Vec<Step> {
    let mut configure = vec!["./configure".to_string(), format!("--prefix={prefix}")];
    configure.extend(configure_flags.iter().cloned());
    vec![
        configure,
        vec!["./build.sh".to_string()],
        vec!["./make".to_string(), "install".to_string()],
    ]
}

/// Steps of the `make` build kind, for packages that ship a Makefile and no
/// configure script.
///
/// 1. `["make", "-j<jobs>", <make_flags>..., <build_targets>...]`
/// 2. only when `check`: `["make", "-j<jobs>", "check", <make_flags>...]`
/// 3. `["make", <install_targets>..., <make_flags>..., <install_flags>...]`,
///    where empty `install_targets` means `install`
pub fn make_steps(
    make_flags: &[String],
    install_flags: &[String],
    build_targets: &[String],
    install_targets: &[String],
    check_target: Option<&str>,
    jobs: u32,
) -> Vec<Step> {
    let jobs = format!("-j{jobs}");
    let mut build = vec!["make".to_string(), jobs.clone()];
    build.extend(make_flags.iter().cloned());
    build.extend(build_targets.iter().cloned());
    let mut steps = vec![build];
    if let Some(target) = check_target {
        let mut step = vec!["make".to_string(), jobs, target.to_owned()];
        step.extend(make_flags.iter().cloned());
        steps.push(step);
    }
    let mut install = vec!["make".to_string()];
    if install_targets.is_empty() {
        install.push("install".to_string());
    } else {
        install.extend(install_targets.iter().cloned());
    }
    install.extend(make_flags.iter().cloned());
    install.extend(install_flags.iter().cloned());
    steps.push(install);
    steps
}

/// Steps of the `cmake` build kind.
///
/// 1. `["cmake", "-S", <path>, "-B", "build", "-G", "Unix Makefiles",
///    "-DCMAKE_INSTALL_PREFIX=<prefix>", "-DCMAKE_INSTALL_LIBDIR=lib",
///    "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_INSTALL_RPATH=<prefix>/lib",
///    <setup_flags>...]`
/// 2. `["cmake", "--build", "build", "-j", "<jobs>"]`
/// 3. only when `check`: `["ctest", "--test-dir", "build", "-j", "<jobs>"]`
/// 4. `["cmake", "--install", "build"]`
pub fn cmake_steps(
    prefix: &str,
    path: &str,
    setup_flags: &[String],
    check: bool,
    check_skip: &[String],
    jobs: u32,
) -> Vec<Step> {
    let words = |words: &[&str]| -> Step { words.iter().map(|w| w.to_string()).collect() };
    let mut setup = words(&["cmake", "-S", path, "-B", "build", "-G", "Unix Makefiles"]);
    setup.push(format!("-DCMAKE_INSTALL_PREFIX={prefix}"));
    setup.push("-DCMAKE_INSTALL_LIBDIR=lib".to_string());
    setup.push("-DCMAKE_BUILD_TYPE=Release".to_string());
    setup.push(format!("-DCMAKE_INSTALL_RPATH={prefix}/lib"));
    setup.extend(setup_flags.iter().cloned());
    let mut steps = vec![setup];
    let mut build = words(&["cmake", "--build", "build", "-j"]);
    build.push(jobs.to_string());
    steps.push(build);
    if check {
        let mut test = words(&["ctest", "--test-dir", "build", "-j"]);
        test.push(jobs.to_string());
        if !check_skip.is_empty() {
            let literals: Vec<String> = check_skip
                .iter()
                .map(|name| {
                    let mut escaped = String::new();
                    for c in name.chars() {
                        if ".^$[]()|*+?\\".contains(c) {
                            escaped.push('\\');
                        }
                        escaped.push(c);
                    }
                    escaped
                })
                .collect();
            test.extend(["-E".to_owned(), format!("^({})$", literals.join("|"))]);
        }
        steps.push(test);
    }
    steps.push(words(&["cmake", "--install", "build"]));
    steps
}

pub fn cargo_vendor_steps(crates: &[(String, String, String)]) -> Vec<Step> {
    let words = |words: &[&str]| -> Step { words.iter().map(|w| w.to_string()).collect() };
    let mut steps = vec![words(&[
        "mkdir",
        "-p",
        "/build/vendor",
        "/build/cargo-home",
    ])];
    for (name, version, sha256) in crates {
        let dir = format!("/build/vendor/{name}-{version}");
        steps.push(vec!["mkdir".to_string(), "-p".to_string(), dir.clone()]);
        steps.push(vec![
            "tar".to_string(),
            "-xzf".to_string(),
            format!("/build/crates/{name}-{version}.crate"),
            "--strip-components=1".to_string(),
            "-C".to_string(),
            dir.clone(),
        ]);
        steps.push(vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' \"$2\" > \"$1\"".to_string(),
            "sh".to_string(),
            format!("{dir}/.cargo-checksum.json"),
            format!("{{\"files\":{{}},\"package\":\"{sha256}\"}}"),
        ]);
    }
    let mut config = words(&[
        "sh",
        "-c",
        "printf '%s\\n' \"$@\" > /build/cargo-home/config.toml",
        "sh",
        "[source.crates-io]",
        "replace-with = \"vendored\"",
        "[source.vendored]",
        "directory = \"/build/vendor\"",
        "[target.x86_64-unknown-linux-musl]",
        "linker = \"cc\"",
        "rustflags = [\"-Clink-self-contained=no\", \"-Ctarget-feature=-crt-static\"]",
    ]);
    config.shrink_to_fit();
    steps.push(config);
    steps
}

/// Steps of the `cargo` build kind, run in the unpacked source after the
/// crates `(name, version, sha256)` are at `/build/crates/<name>-<version>.crate`.
///
/// 1. `["rm", "-rf", ".cargo"]`
/// 2. per crate: unpack it to `/build/vendor/<name>-<version>` and write its
///    `.cargo-checksum.json`
/// 3. write `/build/cargo-home/config.toml`: the vendor directory replaces
///    crates.io, the linker is `cc`, and programs link against the C
///    library the linker chooses
/// 4. `["env", "CARGO_HOME=/build/cargo-home", "cargo", "install", "--path",
///    <path>, "--root", <prefix>, "--offline", "--locked", "--no-track",
///    "--jobs", "<jobs>", <flags>...]`
pub fn cargo_steps(
    prefix: &str,
    path: &str,
    flags: &[String],
    check_skip: &[String],
    jobs: u32,
    crates: &[(String, String, String)],
) -> Vec<Step> {
    let words = |words: &[&str]| -> Step { words.iter().map(|w| w.to_string()).collect() };
    let mut steps = vec![words(&["rm", "-rf", ".cargo"])];
    steps.extend(cargo_vendor_steps(crates));
    if !check_skip.is_empty() {
        let mut test = words(&[
            "env",
            "CARGO_HOME=/build/cargo-home",
            "cargo",
            "test",
            "--manifest-path",
        ]);
        test.push(format!("{path}/Cargo.toml"));
        test.extend(words(&["--release", "--offline", "--locked", "--jobs"]));
        test.push(jobs.to_string());
        test.extend(words(&["--", "--exact"]));
        for name in check_skip {
            test.push("--skip".to_owned());
            test.push(name.clone());
        }
        steps.push(test);
    }
    let mut install = words(&[
        "env",
        "CARGO_HOME=/build/cargo-home",
        "cargo",
        "install",
        "--path",
        path,
        "--root",
        prefix,
        "--offline",
        "--locked",
        "--no-track",
        "--jobs",
    ]);
    install.push(jobs.to_string());
    install.extend(flags.iter().cloned());
    steps.push(install);
    steps
}

/// Steps of the `meson` build kind.
///
/// 1. `["meson", "setup", "build", "--prefix=<prefix>", "--libdir=lib",
///    "--buildtype=release", <setup_flags>...]`
/// 2. `["meson", "compile", "-C", "build", "-j", "<jobs>"]`
/// 3. only when `check`: `["meson", "test", "-C", "build", "--no-rebuild"]`
/// 4. `["meson", "install", "-C", "build", "--no-rebuild", "--destdir",
///    "/build/dest"]`
/// 5. Copy the staged prefix to the output and the remaining files to
///    `<prefix>/.vug/root`.
pub fn meson_steps(
    prefix: &str,
    setup_flags: &[String],
    check: bool,
    check_skip: &[String],
    jobs: u32,
) -> Vec<Step> {
    let words = |words: &[&str]| -> Step { words.iter().map(|w| w.to_string()).collect() };
    let mut setup = words(&["meson", "setup", "build"]);
    setup.push(format!("--prefix={prefix}"));
    setup.push("--libdir=lib".to_string());
    setup.push("--buildtype=release".to_string());
    setup.extend(setup_flags.iter().cloned());
    let mut steps = vec![setup];
    let mut compile = words(&["meson", "compile", "-C", "build", "-j"]);
    compile.push(jobs.to_string());
    steps.push(compile);
    if check {
        let mut test = words(&["meson", "test", "-C", "build", "--no-rebuild"]);
        if !check_skip.is_empty() {
            test.splice(..0, words(&["python3", "-c", MESON_CHECK]));
            test.extend(check_skip.iter().cloned());
        }
        steps.push(test);
    }
    steps.push(words(&["mkdir", "-p", INSTALL_STAGE]));
    steps.push(words(&[
        "meson",
        "install",
        "-C",
        "build",
        "--no-rebuild",
        "--destdir",
        INSTALL_STAGE,
    ]));
    steps.push(collect_staged_install(prefix));
    steps
}

const MESON_CHECK: &str = r#"
import json
import subprocess
import sys

command, skip = sys.argv[1:6], set(sys.argv[6:])
tests = json.loads(subprocess.check_output(["meson", "introspect", "--tests", "build"]))
names = {test["name"] for test in tests}
missing = skip - names
if missing:
    sys.exit("check.skip names unknown Meson tests: " + ", ".join(sorted(missing)))
escape = str.maketrans({"*": "[*]", "?": "[?]", "[": "[[]"})
selected = [name.translate(escape) for name in sorted(names - skip)]
if selected:
    sys.exit(subprocess.call(command + ["--", *selected]))
print("No Meson tests selected")
"#;

fn collect_staged_install(prefix: &str) -> Step {
    [
        "sh",
        "-ec",
        r#"
stage=$1
prefix=$2
staged_prefix=$stage$prefix
if [ -e "$staged_prefix" ] || [ -L "$staged_prefix" ]; then
    cp -a "$staged_prefix/." "$prefix/"
    rm -rf "$staged_prefix"
    parent=${staged_prefix%/*}
    while [ "$parent" != "$stage" ] && rmdir "$parent" 2>/dev/null; do
        parent=${parent%/*}
    done
fi
mkdir -p "$prefix/.vug/root"
cp -a "$stage/." "$prefix/.vug/root/"
"#,
        "sh",
        INSTALL_STAGE,
        prefix,
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Steps of the `linux` build kind.
///
/// Every `make` call carries the tool names of the seed, the flags for the
/// programs that run during the build, and a fixed build identity:
/// `LLVM_IAS=1 CC=cc HOSTCC=cc HOSTCXX=c++ LD=/bin/ld HOSTLD=/bin/ld AR=ar
/// NM=nm OBJCOPY=objcopy OBJDUMP=objdump STRIP=strip READELF=readelf
/// HOSTCFLAGS=<host_cflags> HOSTLDFLAGS=<host_ldflags>
/// KBUILD_BUILD_TIMESTAMP=1980-01-01 KBUILD_BUILD_USER=vug
/// KBUILD_BUILD_HOST=vug`.
///
/// 1. `make ... <defconfig>`
/// 2. for each `NAME=value` in `config`, in order: `bash scripts/config`
///    with `--enable NAME` for `y`, `--disable NAME` for `n`, `--module NAME`
///    for `m`, and `--set-val NAME value` otherwise
/// 3. `make ... olddefconfig`
/// 4. `make ... -j<jobs> bzImage`
/// 5. `mkdir -p <prefix>/boot`, then copy `arch/x86/boot/bzImage` to
///    `<prefix>/boot/bzImage` and `.config` to `<prefix>/boot/config`
pub fn linux_steps(
    prefix: &str,
    defconfig: &str,
    config: &[String],
    jobs: u32,
    host_cflags: &str,
    host_ldflags: &str,
    module_key: Option<&str>,
) -> Vec<Step> {
    let make = |words: &[&str]| -> Step {
        let mut step: Step = [
            "make",
            "LLVM_IAS=1",
            "CC=cc",
            "HOSTCC=cc",
            "HOSTCXX=c++",
            "LD=/bin/ld",
            "HOSTLD=/bin/ld",
            "AR=ar",
            "NM=nm",
            "OBJCOPY=objcopy",
            "OBJDUMP=objdump",
            "STRIP=strip",
            "READELF=readelf",
        ]
        .iter()
        .map(|w| w.to_string())
        .collect();
        step.push(format!("HOSTCFLAGS={host_cflags}"));
        step.push(format!("HOSTLDFLAGS={host_ldflags}"));
        step.push("KBUILD_BUILD_TIMESTAMP=1980-01-01".to_string());
        step.push("KBUILD_BUILD_USER=vug".to_string());
        step.push("KBUILD_BUILD_HOST=vug".to_string());
        if let Some(key) = module_key {
            step.push(format!("LOCALVERSION=-vug-{key}"));
        }
        step.extend(words.iter().map(|w| w.to_string()));
        step
    };
    let mut steps = vec![make(&[defconfig])];
    for entry in config {
        let (name, value) = entry.split_once('=').unwrap_or((entry, "y"));
        let mut step = vec!["bash".to_string(), "scripts/config".to_string()];
        match value {
            "y" => step.extend(["--enable".to_string(), name.to_string()]),
            "n" => step.extend(["--disable".to_string(), name.to_string()]),
            "m" => step.extend(["--module".to_string(), name.to_string()]),
            _ => step.extend(["--set-val".to_string(), name.to_string(), value.to_string()]),
        }
        steps.push(step);
    }
    steps.push(make(&["olddefconfig"]));
    steps.push(make(&[&format!("-j{jobs}"), "bzImage"]));
    if module_key.is_some() {
        steps.push(make(&[&format!("-j{jobs}"), "modules"]));
        steps.push(make(&[
            &format!("INSTALL_MOD_PATH={prefix}"),
            "DEPMOD=true",
            "modules_install",
        ]));
        steps.push(vec![
            "bash".into(),
            "-ec".into(),
            r#"release=$(cat include/config/kernel.release)
dir="$1/lib/modules/$release"
test -d "$dir/kernel"
rm -f "$dir/build" "$dir/source"
depmod -b "$1" "$release"
test -s "$dir/modules.dep.bin""#
                .into(),
            "vug-modules".into(),
            prefix.into(),
        ]);
    }
    steps.push(vec![
        "mkdir".to_string(),
        "-p".to_string(),
        format!("{prefix}/boot"),
    ]);
    steps.push(vec![
        "cp".to_string(),
        "arch/x86/boot/bzImage".to_string(),
        format!("{prefix}/boot/bzImage"),
    ]);
    steps.push(vec![
        "cp".to_string(),
        ".config".to_string(),
        format!("{prefix}/boot/config"),
    ]);
    steps
}

/// Steps of the `prebuilt` build kind. For each `(from, to)` in order:
///
/// 1. `["mkdir", "-p", "<parent directory of prefix/to>"]`
/// 2. `["cp", "-a", from, "<prefix>/<to>"]`
///
/// The parent directory of `<prefix>/<to>` is everything before its last
/// `/`. `to` never starts with `/` and never ends with `/`.
pub fn prebuilt_steps(prefix: &str, install: &[(String, String)]) -> Vec<Step> {
    let mut steps = Vec::new();
    for (from, to) in install {
        let dest = format!("{prefix}/{to}");
        let parent = dest.rsplit_once('/').map_or(dest.as_str(), |(p, _)| p);
        steps.push(vec![
            "mkdir".to_string(),
            "-p".to_string(),
            parent.to_string(),
        ]);
        steps.push(vec!["cp".to_string(), "-a".to_string(), from.clone(), dest]);
    }
    steps
}

/// Renders steps as a POSIX shell script.
///
/// The script is `set -eu\n` followed by one line per step. A line is the
/// words of the step joined by single spaces, each word quoted by
/// `shell_quote`, and ends with `\n`.
pub fn shell_script(steps: &[Step]) -> String {
    let mut script = String::from("set -eu\n");
    for step in steps {
        for (i, word) in step.iter().enumerate() {
            if i > 0 {
                script.push(' ');
            }
            script.push_str(&shell_quote(word));
        }
        script.push('\n');
    }
    script
}

/// Quotes one word for a POSIX shell.
///
/// A non-empty word made only of `A-Z a-z 0-9` and `_ - . / = : + , @ %` is
/// returned unchanged. Any other word is wrapped in single quotes, with each
/// `'` inside it written as `'\''`. The empty word is `''`.
pub fn shell_quote(word: &str) -> String {
    fn safe(c: char) -> bool {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '-' | '.' | '/' | '=' | ':' | '+' | ',' | '@' | '%')
    }
    if !word.is_empty() && word.chars().all(safe) {
        return word.to_string();
    }
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for c in word.chars() {
        if c == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

/// Arguments for `bwrap` (the program name itself is not included).
///
/// In this order:
///
/// 1. `--unshare-all`, `--die-with-parent`, `--clearenv`
/// 2. `--proc`, `/proc`, `--dev`, `/dev`, `--tmpfs`, `/tmp`
/// 3. for each bind in order: `--bind` (writable) or `--ro-bind`, source, dest
/// 4. for each env pair in order: `--setenv`, name, value
/// 5. `--remount-ro`, `/`: the root itself is read-only, so a build can write
///    only to the writable binds and to `/tmp`
/// 6. `--chdir`, workdir
/// 7. `--`, then the command words
pub fn bwrap_args(sandbox: &Sandbox) -> Vec<String> {
    let mut args = vec![
        "--unshare-all".to_string(),
        "--hostname".to_string(),
        "vug".to_string(),
        "--die-with-parent".to_string(),
        "--clearenv".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
    ];
    for bind in &sandbox.binds {
        args.push(if bind.writable { "--bind" } else { "--ro-bind" }.to_string());
        args.push(bind.source.clone());
        args.push(bind.dest.clone());
    }
    for (name, value) in &sandbox.env {
        args.push("--setenv".to_string());
        args.push(name.clone());
        args.push(value.clone());
    }
    args.push("--remount-ro".to_string());
    args.push("/".to_string());
    args.push("--chdir".to_string());
    args.push(sandbox.workdir.clone());
    args.push("--".to_string());
    args.extend(sandbox.command.iter().cloned());
    args
}
