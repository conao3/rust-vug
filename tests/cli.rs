use std::process::{Command, Output};

fn vug(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vug"))
        .args(args)
        .output()
        .expect("run vug")
}

fn compare(left: &str, right: &str) -> Output {
    vug(&[
        "version",
        "compare-versions",
        "--left",
        left,
        "--right",
        right,
    ])
}

fn ordering(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    value["Ordering"].as_str().expect("Ordering key").to_owned()
}

#[test]
fn compare_versions_json() {
    for (left, right, expected) in [
        ("1.2.10", "1.2.9", "Greater"),
        ("1.0", "1.00", "Equal"),
        ("1.0rc1", "1.0", "Less"),
        ("1.0.1", "1.0", "Greater"),
        ("1.0a", "1.0.1", "Less"),
        ("2.0", "1.99.99", "Greater"),
        ("1.0-beta", "1.0-rc", "Less"),
        ("", "", "Equal"),
        ("1_2", "1.2", "Equal"),
    ] {
        assert_eq!(
            ordering(&compare(left, right)),
            expected,
            "{left} vs {right}"
        );
    }
}

#[test]
fn compare_versions_json_shape() {
    let output = compare("1.2.10", "1.2.9");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "{\n  \"Ordering\": \"Greater\"\n}\n"
    );
}

#[test]
fn compare_versions_text_output() {
    let output = vug(&[
        "--output",
        "text",
        "version",
        "compare-versions",
        "--left",
        "1.2.10",
        "--right",
        "1.2.9",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "Greater\n");
}

#[test]
fn compare_versions_table_output() {
    let output = vug(&[
        "--output",
        "table",
        "version",
        "compare-versions",
        "--left",
        "1.2.10",
        "--right",
        "1.2.9",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Ordering\nGreater\n"
    );
}

#[test]
fn file_input() {
    let path = std::env::temp_dir().join(format!("vug-test-left-{}", std::process::id()));
    std::fs::write(&path, "1.2.10").unwrap();
    let arg = format!("file://{}", path.display());
    assert_eq!(
        ordering(&vug(&[
            "version",
            "compare-versions",
            "--left",
            &arg,
            "--right",
            "1.2.9"
        ])),
        "Greater"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_parameter_is_usage_error() {
    let output = vug(&["version", "compare-versions", "--left", "1.0"]);
    assert_eq!(output.status.code(), Some(252));
    assert!(output.stdout.is_empty());
}

#[test]
fn unknown_option_is_usage_error() {
    let output = vug(&["version", "compare-versions", "--bogus", "x"]);
    assert_eq!(output.status.code(), Some(252));
}

#[test]
fn missing_file_is_failure() {
    let output = vug(&[
        "version",
        "compare-versions",
        "--left",
        "file:///nonexistent/path",
        "--right",
        "1.0",
    ]);
    assert_eq!(output.status.code(), Some(254));
}

#[test]
fn help_at_every_level() {
    for args in [
        vec!["help"],
        vec!["--help"],
        vec!["version", "help"],
        vec!["version", "compare-versions", "help"],
        vec!["version", "compare-versions", "--help"],
    ] {
        let output = vug(&args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage"));
    }
}

#[test]
fn version_flag() {
    let output = vug(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn debug_flag() {
    let output = vug(&[
        "--debug",
        "version",
        "compare-versions",
        "--left",
        "1.0",
        "--right",
        "1.1",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("left"));
}

fn system_error(record: &str) -> String {
    let root = tempfile::tempdir().expect("temporary directory");
    let systems = root.path().join("systems");
    std::fs::create_dir(&systems).expect("create systems");
    std::fs::write(systems.join("demo.toml"), record).expect("write record");
    let output = vug(&[
        "system",
        "build-system",
        "--root",
        root.path().to_str().expect("path"),
        "--name",
        "demo",
        "--data-dir",
        root.path().join("data").to_str().expect("path"),
        "--cache-dir",
        root.path().join("cache").to_str().expect("path"),
    ]);
    assert_ne!(output.status.code(), Some(0));
    String::from_utf8(output.stderr).expect("stderr is UTF-8")
}

const SYSTEM_HEAD: &str =
    "name = \"demo\"\nkernel = \"linux\"\ninit = \"/bin/sh\"\npackages = []\n";

#[test]
fn system_record_errors() {
    for (body, expected) in [
        (
            "[[file]]\npath = \"/init\"\ncontent = \"\"\n",
            "expected a path below /etc",
        ),
        (
            "[[file]]\npath = \"/etc/a\"\ncontent = \"\"\n[[file]]\npath = \"/etc/a/b\"\ncontent = \"\"\n",
            "inside another listed path",
        ),
        (
            "[[file]]\npath = \"/etc/a\"\nmode = \"600\"\ncontent = \"\"\n",
            "a link file takes",
        ),
        (
            "[[file]]\npath = \"/etc/a\"\nkind = \"copy\"\ncontent = \"\"\n",
            "expected \"link\" or \"seed\"",
        ),
        (
            "[[group]]\nname = \"root\"\ngid = 0\n[[user]]\nname = \"root\"\nuid = 0\ngroup = \"wheel\"\nhome = \"/root\"\nshell = \"/bin/sh\"\n",
            "is not a group of this system",
        ),
        (
            "[[group]]\nname = \"root\"\ngid = 0\n[[user]]\nname = \"root\"\nuid = 0\ngroup = \"root\"\nhome = \"/root\"\nshell = \"/bin/sh\"\n[[file]]\npath = \"/etc/passwd\"\ncontent = \"\"\n",
            "written by vug itself",
        ),
        (
            "[[setuid]]\npackage = \"sudo\"\nprogram = \"bin/sudo\"\n",
            "is not a package of this system",
        ),
    ] {
        let error = system_error(&format!("{SYSTEM_HEAD}{body}"));
        assert!(error.contains(expected), "{body:?}: {error}");
    }
}

#[test]
fn system_record_needs_init() {
    let error = system_error("name = \"demo\"\nkernel = \"linux\"\npackages = []\n");
    assert!(error.contains("init: expected a string"), "{error}");
}
