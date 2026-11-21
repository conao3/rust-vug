//! Build steps and the sandbox command line. Every function here is pure:
//! it only builds lists of strings.

/// A build step: a program and its arguments.
pub type Step = Vec<String>;

pub struct Autotools {
    /// The words that start the configure step; empty means
    /// `./configure --prefix=<prefix>`.
    pub configure_command: Vec<String>,
    pub configure_flags: Vec<String>,
    pub make_flags: Vec<String>,
    pub install_flags: Vec<String>,
    /// Targets of the install step; empty means `install`.
    pub install_targets: Vec<String>,
    /// Whether `make check` runs.
    pub check: bool,
    /// Parallel jobs passed as `-j<jobs>`.
    pub jobs: u32,
}

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
/// 1. `["tar", "-xf", archive, "--strip-components=<strip>"]`
/// 2. for each patch, in order: `["patch", "-p1", "-i", patch]`
pub fn unpack_steps(archive: &str, strip: u32, patches: &[String]) -> Vec<Step> {
    let mut steps = vec![vec![
        "tar".to_string(),
        "-xf".to_string(),
        archive.to_string(),
        format!("--strip-components={strip}"),
    ]];
    for patch in patches {
        steps.push(vec![
            "patch".to_string(),
            "-p1".to_string(),
            "-i".to_string(),
            patch.clone(),
        ]);
    }
    steps
}

/// Steps of the `autotools` build kind.
///
/// 1. `["./configure", "--prefix=<prefix>", <configure_flags>...]`
/// 2. `["make", "-j<jobs>", <make_flags>...]`
/// 3. only when `check`: `["make", "-j<jobs>", "check", <make_flags>...]`
/// 4. `["make", "install", <make_flags>..., <install_flags>...]`
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

    let mut steps = vec![configure, make];

    if options.check {
        let mut check = vec!["make".to_string(), jobs, "check".to_string()];
        check.extend(options.make_flags.iter().cloned());
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
    steps.push(install);

    steps
}

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
    check: bool,
    jobs: u32,
) -> Vec<Step> {
    let jobs = format!("-j{jobs}");
    let mut build = vec!["make".to_string(), jobs.clone()];
    build.extend(make_flags.iter().cloned());
    build.extend(build_targets.iter().cloned());
    let mut steps = vec![build];
    if check {
        let mut step = vec!["make".to_string(), jobs, "check".to_string()];
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
/// 1. `["cmake", "-S", ".", "-B", "build", "-G", "Unix Makefiles",
///    "-DCMAKE_INSTALL_PREFIX=<prefix>", "-DCMAKE_INSTALL_LIBDIR=lib",
///    "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON",
///    "-DCMAKE_INSTALL_RPATH=<prefix>/lib", <setup_flags>...]`
/// 2. `["cmake", "--build", "build", "-j", "<jobs>"]`
/// 3. only when `check`: `["ctest", "--test-dir", "build", "-j", "<jobs>"]`
/// 4. `["cmake", "--install", "build"]`
pub fn cmake_steps(prefix: &str, setup_flags: &[String], check: bool, jobs: u32) -> Vec<Step> {
    let words = |words: &[&str]| -> Step { words.iter().map(|w| w.to_string()).collect() };
    let mut setup = words(&["cmake", "-S", ".", "-B", "build", "-G", "Unix Makefiles"]);
    setup.push(format!("-DCMAKE_INSTALL_PREFIX={prefix}"));
    setup.push("-DCMAKE_INSTALL_LIBDIR=lib".to_string());
    setup.push("-DCMAKE_BUILD_TYPE=Release".to_string());
    setup.push("-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON".to_string());
    setup.push(format!("-DCMAKE_INSTALL_RPATH={prefix}/lib"));
    setup.extend(setup_flags.iter().cloned());
    let mut steps = vec![setup];
    let mut build = words(&["cmake", "--build", "build", "-j"]);
    build.push(jobs.to_string());
    steps.push(build);
    if check {
        let mut test = words(&["ctest", "--test-dir", "build", "-j"]);
        test.push(jobs.to_string());
        steps.push(test);
    }
    steps.push(words(&["cmake", "--install", "build"]));
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
/// 5. `["cp", "-a", "/build/dest<prefix>/.", "<prefix>/"]`, then
///    `["rm", "-rf", "/build/dest/vug"]`
/// 6. `["mkdir", "-p", "<prefix>/.vug/root"]`, then
///    `["cp", "-a", "/build/dest/.", "<prefix>/.vug/root/"]`: what the
///    package installs outside its prefix (`/etc`, `/var`) is kept under
///    `.vug/root` of the output
pub fn meson_steps(prefix: &str, setup_flags: &[String], check: bool, jobs: u32) -> Vec<Step> {
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
        steps.push(words(&["meson", "test", "-C", "build", "--no-rebuild"]));
    }
    steps.push(words(&[
        "meson",
        "install",
        "-C",
        "build",
        "--no-rebuild",
        "--destdir",
        "/build/dest",
    ]));
    steps.push(vec![
        "cp".to_string(),
        "-a".to_string(),
        format!("/build/dest{prefix}/."),
        format!("{prefix}/"),
    ]);
    steps.push(words(&["rm", "-rf", "/build/dest/vug"]));
    steps.push(vec![
        "mkdir".to_string(),
        "-p".to_string(),
        format!("{prefix}/.vug/root"),
    ]);
    steps.push(vec![
        "cp".to_string(),
        "-a".to_string(),
        "/build/dest/.".to_string(),
        format!("{prefix}/.vug/root/"),
    ]);
    steps
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
