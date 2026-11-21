use std::process::Command;

use vug::steps::{
    Autotools, Bind, Sandbox, Step, autotools_steps, bwrap_args, prebuilt_steps, shell_quote,
    shell_script, unpack_steps,
};

fn step(words: &[&str]) -> Step {
    words.iter().map(|w| w.to_string()).collect()
}

fn owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn unpack_without_patches() {
    let steps = unpack_steps("src.tar.gz", 1, &[]);
    assert_eq!(
        steps,
        vec![step(&["tar", "-xf", "src.tar.gz", "--strip-components=1"])]
    );
}

#[test]
fn unpack_with_patches() {
    let patches = owned(&["x.patch", "y.patch"]);
    let steps = unpack_steps("a.tar.xz", 0, &patches);
    assert_eq!(
        steps,
        vec![
            step(&["tar", "-xf", "a.tar.xz", "--strip-components=0"]),
            step(&["patch", "-p1", "-i", "x.patch"]),
            step(&["patch", "-p1", "-i", "y.patch"]),
        ]
    );
}

#[test]
fn autotools_with_check() {
    let options = Autotools {
        configure_flags: owned(&["--disable-nls"]),
        make_flags: owned(&["V=1"]),
        install_flags: owned(&["DESTDIR="]),
        check: true,
        jobs: 4,
    };
    let steps = autotools_steps("/out", &options);
    assert_eq!(
        steps,
        vec![
            step(&["./configure", "--prefix=/out", "--disable-nls"]),
            step(&["make", "-j4", "V=1"]),
            step(&["make", "-j4", "check", "V=1"]),
            step(&["make", "install", "V=1", "DESTDIR="]),
        ]
    );
}

#[test]
fn autotools_without_check() {
    let options = Autotools {
        configure_flags: owned(&["--disable-nls"]),
        make_flags: owned(&["V=1"]),
        install_flags: owned(&["DESTDIR="]),
        check: false,
        jobs: 4,
    };
    let steps = autotools_steps("/out", &options);
    assert_eq!(
        steps,
        vec![
            step(&["./configure", "--prefix=/out", "--disable-nls"]),
            step(&["make", "-j4", "V=1"]),
            step(&["make", "install", "V=1", "DESTDIR="]),
        ]
    );
    assert_eq!(steps.len(), 3);
    assert!(!steps.iter().flatten().any(|w| w == "check"));
}

#[test]
fn autotools_minimal() {
    let options = Autotools {
        configure_flags: vec![],
        make_flags: vec![],
        install_flags: vec![],
        check: false,
        jobs: 1,
    };
    let steps = autotools_steps("/out", &options);
    assert_eq!(
        steps,
        vec![
            step(&["./configure", "--prefix=/out"]),
            step(&["make", "-j1"]),
            step(&["make", "install"]),
        ]
    );
}

#[test]
fn prebuilt() {
    let install = vec![
        ("zig".to_string(), "bin/zig".to_string()),
        ("lib".to_string(), "lib".to_string()),
    ];
    let steps = prebuilt_steps("/out", &install);
    assert_eq!(
        steps,
        vec![
            step(&["mkdir", "-p", "/out/bin"]),
            step(&["cp", "-a", "zig", "/out/bin/zig"]),
            step(&["mkdir", "-p", "/out"]),
            step(&["cp", "-a", "lib", "/out/lib"]),
        ]
    );
}

#[test]
fn quote() {
    assert_eq!(shell_quote("abc"), "abc");
    assert_eq!(shell_quote("--prefix=/out"), "--prefix=/out");
    assert_eq!(shell_quote("a b"), "'a b'");
    assert_eq!(shell_quote(""), "''");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote("$HOME"), "'$HOME'");
    assert_eq!(shell_quote("a;b"), "'a;b'");
    assert_eq!(shell_quote("a*"), "'a*'");
    assert_eq!(shell_quote("a\nb"), "'a\nb'");
}

#[test]
fn script() {
    let steps = vec![step(&["make", "-j4"]), step(&["echo", "a b"])];
    assert_eq!(shell_script(&steps), "set -eu\nmake -j4\necho 'a b'\n");
}

#[test]
fn script_empty() {
    assert_eq!(shell_script(&[]), "set -eu\n");
}

#[test]
fn script_runs_under_sh() {
    let steps = vec![step(&["printf", "%s\n", "it's $HOME; ok"])];
    let output = Command::new("sh")
        .arg("-c")
        .arg(shell_script(&steps))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"it's $HOME; ok\n");
}

#[test]
fn bwrap_full() {
    let sandbox = Sandbox {
        binds: vec![
            Bind {
                source: "/s".to_string(),
                dest: "/seed".to_string(),
                writable: false,
            },
            Bind {
                source: "/w".to_string(),
                dest: "/build".to_string(),
                writable: true,
            },
        ],
        env: vec![
            ("PATH".to_string(), "/bin".to_string()),
            ("HOME".to_string(), "/build".to_string()),
        ],
        workdir: "/build".to_string(),
        command: owned(&["sh", "build.sh"]),
    };
    assert_eq!(
        bwrap_args(&sandbox),
        owned(&[
            "--unshare-all",
            "--die-with-parent",
            "--clearenv",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--ro-bind",
            "/s",
            "/seed",
            "--bind",
            "/w",
            "/build",
            "--setenv",
            "PATH",
            "/bin",
            "--setenv",
            "HOME",
            "/build",
            "--chdir",
            "/build",
            "--",
            "sh",
            "build.sh",
        ])
    );
}

#[test]
fn bwrap_minimal() {
    let sandbox = Sandbox {
        binds: vec![],
        env: vec![],
        workdir: "/build".to_string(),
        command: owned(&["true"]),
    };
    assert_eq!(
        bwrap_args(&sandbox),
        owned(&[
            "--unshare-all",
            "--die-with-parent",
            "--clearenv",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--chdir",
            "/build",
            "--",
            "true",
        ])
    );
}

#[test]
fn bootstrap_make() {
    assert_eq!(
        vug::steps::bootstrap_make_steps("/out", &owned(&["--disable-nls"])),
        vec![
            step(&["./configure", "--prefix=/out", "--disable-nls"]),
            step(&["./build.sh"]),
            step(&["./make", "install"]),
        ]
    );
}
