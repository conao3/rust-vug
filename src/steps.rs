//! Build steps and the sandbox command line. Every function here is pure:
//! it only builds lists of strings.

/// A build step: a program and its arguments.
pub type Step = Vec<String>;

pub struct Autotools {
    pub configure_flags: Vec<String>,
    pub make_flags: Vec<String>,
    pub install_flags: Vec<String>,
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
    let mut configure = vec!["./configure".to_string(), format!("--prefix={prefix}")];
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

    let mut install = vec!["make".to_string(), "install".to_string()];
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
/// 5. `--chdir`, workdir
/// 6. `--`, then the command words
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
    args.push("--chdir".to_string());
    args.push(sandbox.workdir.clone());
    args.push("--".to_string());
    args.extend(sandbox.command.iter().cloned());
    args
}
