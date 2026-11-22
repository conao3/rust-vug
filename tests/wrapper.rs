//! The compiler wrapper, run with a stand-in for zig that prints the
//! arguments it receives, one per line.

use std::process::Command;

fn run(directory: &std::path::Path, arguments: &[&str]) -> Vec<String> {
    run_as(directory, "cc", "c", None, arguments)
}

fn run_as(
    directory: &std::path::Path,
    tool: &str,
    lang: &str,
    libcxx: Option<&str>,
    arguments: &[&str],
) -> Vec<String> {
    let zig = directory.join("zig");
    std::fs::write(&zig, "#!/bin/sh\nfor a; do printf '%s\\n' \"$a\"; done\n").expect("write");
    let wrapper = directory.join("cc");
    let text = vug::wrapper::COMPILER_WRAPPER
        .replace("/seed/zig/zig", &format!("sh {}", zig.display()))
        .replace("{tool}", tool)
        .replace("{lang}", lang);
    std::fs::write(&wrapper, text).expect("write");
    let mut command = Command::new("sh");
    command
        .arg(&wrapper)
        .args(arguments)
        .current_dir(directory)
        .env_remove("VUG_CFLAGS")
        .env_remove("VUG_LDFLAGS")
        .env_remove("VUG_LIBCXX");
    if let Some(libcxx) = libcxx {
        command.env("VUG_LIBCXX", libcxx);
    }
    let output = command.output().expect("run wrapper");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The arguments after the fixed head `cc -target ... -UNDEBUG`, without
/// the two `-Wno-error=` flags the wrapper always adds.
fn translated(arguments: &[&str]) -> Vec<String> {
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(directory.path().join("a.c"), "").expect("write");
    let all = run(directory.path(), arguments);
    assert_eq!(
        &all[..5],
        [
            "cc",
            "-target",
            "x86_64-linux-musl",
            "-fno-sanitize=undefined",
            "-UNDEBUG"
        ]
    );
    all[5..]
        .iter()
        .filter(|a| !a.starts_with("-Wno-error="))
        .cloned()
        .collect()
}

#[test]
fn rpath_in_every_spelling() {
    for (given, expected) in [
        (vec!["-Wl,-rpath,/a"], vec!["-Wl,-rpath,/a"]),
        (vec!["-Wl,-rpath=/a"], vec!["-Wl,-rpath=/a"]),
        (vec!["-Wl,-rpath", "-Wl,/a"], vec!["-Wl,-rpath,/a"]),
        (vec!["-Wl,-rpath", "/a"], vec!["-Wl,-rpath,/a"]),
        (vec!["-Wl,-R/a"], vec!["-Wl,-rpath,/a"]),
        (vec!["-Wl,-R", "-Wl,/a"], vec!["-Wl,-rpath,/a"]),
        (
            vec!["-Wl,-rpath", "-Wl,/a", "-Wl,-rpath", "-Wl,/b"],
            vec!["-Wl,-rpath,/a", "-Wl,-rpath,/b"],
        ),
        (
            vec![
                "-Wl,--as-needed,-rpath",
                "-Wl,/a",
                "-Wl,-soname",
                "-Wl,x.so.1",
            ],
            vec!["-Wl,--as-needed", "-Wl,-rpath,/a", "-Wl,-soname,x.so.1"],
        ),
        (
            vec!["-Wl,--version-script", "f.sym"],
            vec!["-Wl,--version-script,f.sym"],
        ),
        (
            vec!["-Wl,--version-script", "-Wl,f.sym"],
            vec!["-Wl,--version-script,f.sym"],
        ),
        (
            vec!["-Wl,--version-script,f.sym"],
            vec!["-Wl,--version-script,f.sym"],
        ),
        (vec!["-Wl,-h", "x.so.1"], vec!["-Wl,-soname,x.so.1"]),
    ] {
        let mut arguments = vec!["a.c", "-o", "a"];
        arguments.extend(&given);
        let mut wanted = vec!["a.c", "-o", "a"];
        wanted.extend(&expected);
        assert_eq!(translated(&arguments), wanted, "{given:?}");
    }
}

#[test]
fn linker_words_that_zig_rejects() {
    assert_eq!(
        translated(&[
            "a.c",
            "-o",
            "a",
            "-Wl,-stats,-lc,-shared,-Map,a.map,-h,x.so.1"
        ]),
        ["a.c", "-o", "a", "-Wl,-soname,x.so.1"]
    );
    assert_eq!(
        translated(&["a.c", "-o", "a", "-Wl,--allow-multiple-definition"]),
        ["a.c", "-o", "a"]
    );
}

#[test]
fn compile_only_spellings() {
    assert_eq!(
        translated(&[
            "-c",
            "a.c",
            "--target=x",
            "-mtune=generic",
            "-march=x86-64-v3"
        ]),
        ["-c", "a.c", "-march=x86_64_v3"]
    );
    assert_eq!(
        translated(&["-c", "a.c", "-isystem", "/i", "-isystem/j"]),
        ["-c", "a.c", "-I/i", "-I/j"]
    );
    assert_eq!(translated(&["-E", "-"]), ["-x", "c", "-E", "-"]);
}

#[test]
fn syntax_only_becomes_a_compile() {
    let got = translated(&["-fsyntax-only", "-x", "c++-header", "a.c"]);
    assert_eq!(&got[..3], ["-x", "c++", "a.c"]);
    assert_eq!(got[3], "-c");
    assert_eq!(got[4], "-o");
    assert!(got[5].ends_with("/syntax.o"), "{got:?}");
}

#[test]
fn nostdlib_keeps_the_c_library_headers() {
    let got = translated(&["-nostdlib", "-c", "a.c"]);
    assert_eq!(&got[..3], ["-nostdlib", "-c", "a.c"]);
    assert_eq!(got[3], "-isystem");
    assert_eq!(got[4], "/seed/zig/lib/libc/include/x86_64-linux-musl");
    assert_eq!(
        translated(&["-nostdlib", "-nostdinc", "-c", "a.c"]).len(),
        4
    );
}

#[test]
fn init_and_fini_become_a_source_file() {
    let got = translated(&[
        "a.c",
        "-shared",
        "-o",
        "a.so",
        "-Wl,-init,start",
        "-Wl,-fini,stop",
    ]);
    assert_eq!(&got[..4], ["a.c", "-shared", "-o", "a.so"]);
    assert!(got[4].ends_with("/init.c"), "{got:?}");
    let text = std::fs::read_to_string(&got[4]).expect("generated source");
    assert!(
        text.contains("static void vug_init(void) { start(); }"),
        "{text}"
    );
    assert!(
        text.contains("static void vug_fini(void) { stop(); }"),
        "{text}"
    );
}

#[test]
fn objects_with_unknown_names_get_a_known_one() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut object = vec![0x7f, b'E', b'L', b'F'];
    object.resize(16, 0);
    object.push(1);
    object.resize(64, 0);
    std::fs::write(directory.path().join("x.oo"), &object).expect("write");
    std::fs::write(directory.path().join("out.oo"), &object).expect("write");
    let got = run(directory.path(), &["x.oo", "-o", "out.oo"]);
    assert!(got[5].ends_with("/1.o"), "{got:?}");
    assert_eq!(got[6..8], ["-o", "out.oo"]);
}

fn cxx(tool: &str, lang: &str, libcxx: Option<&str>, arguments: &[&str]) -> Vec<String> {
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(directory.path().join("a.cpp"), "").expect("write");
    run_as(directory.path(), tool, lang, libcxx, arguments)[5..]
        .iter()
        .filter(|a| !a.starts_with("-Wno-error="))
        .cloned()
        .collect()
}

#[test]
fn shared_cxx_runtime_replaces_the_static_one() {
    let libraries = [
        "/r/lib/libc++.so.1",
        "/r/lib/libc++abi.so.1",
        "/r/lib/libunwind.so.1",
        "-Wl,-rpath,/r/lib",
    ];
    let headers = ["-nostdlib++", "-I/r/include/c++/v1"];
    let mut linked = vec!["a.cpp", "-o", "a"];
    linked.extend(headers);
    linked.extend(libraries);
    assert_eq!(cxx("c++", "c++", Some("/r"), &["a.cpp", "-o", "a"]), linked);
    assert_eq!(
        cxx("c++", "c++", Some("/r"), &["a.cpp", "-lstdc++", "-o", "a"]),
        linked
    );
    let mut compiled = vec!["-c", "a.cpp"];
    compiled.extend(headers);
    assert_eq!(cxx("c++", "c++", Some("/r"), &["-c", "a.cpp"]), compiled);
    let mut driver = vec!["a.o", "-o", "a"];
    driver.extend(libraries);
    assert_eq!(
        cxx("cc", "c", Some("/r"), &["a.o", "-lc++", "-o", "a"]),
        driver
    );
    assert_eq!(
        cxx("cc", "c", Some("/r"), &["a.o", "-o", "a"]),
        ["a.o", "-o", "a"]
    );
}

#[test]
fn cxx_runtime_flags_of_the_caller_are_kept() {
    assert_eq!(
        cxx(
            "c++",
            "c++",
            Some("/r"),
            &["-nostdinc++", "-nostdlib++", "a.cpp", "-o", "a"]
        ),
        ["-nostdinc++", "-nostdlib++", "a.cpp", "-o", "a"]
    );
    assert_eq!(
        cxx("c++", "c++", None, &["a.cpp", "-lc++", "-o", "a"]),
        ["a.cpp", "-lc++", "-o", "a"]
    );
}
