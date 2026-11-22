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

fn system_error_with(record: &str, services: &[(&str, &str)]) -> String {
    let root = tempfile::tempdir().expect("temporary directory");
    for (dir, name, text) in services
        .iter()
        .map(|(name, text)| ("services", *name, *text))
        .chain([("systems", "demo", record)])
    {
        let dir = root.path().join(dir);
        std::fs::create_dir_all(&dir).expect("create directory");
        std::fs::write(dir.join(format!("{name}.toml")), text).expect("write record");
    }
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

fn system_error(record: &str) -> String {
    system_error_with(record, &[])
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
            "[[file]]\npath = \"/etc/localtime\"\ntarget = \"/x\"\ncontent = \"\"\n",
            "goes with a link file without content or mode",
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

#[test]
fn service_record_errors() {
    let getty = "name = \"getty\"\n[parameter.user]\n[parameter.tty]\ndefault = \"ttyS0\"\n[[file]]\npath = \"/etc/getty-{param:tty}\"\ncontent = \"{param:user}\"\n";
    let clash = "name = \"clash\"\n[[file]]\npath = \"/etc/getty-ttyS0\"\ncontent = \"other\"\n";
    let same = "name = \"same\"\n[[file]]\npath = \"/etc/getty-ttyS0\"\ncontent = \"conao\"\n[[file]]\npath = \"/etc/x\"\ncontent = \"{param:none}\"\n";
    for (body, expected) in [
        ("[service.getty]\n", "service.getty: user: needs a value"),
        (
            "[service.getty]\nuser = \"conao\"\nspeed = \"9600\"\n",
            "service.getty: speed: not a parameter of this service",
        ),
        (
            "[service.getty]\nuser = \"conao\"\n[service.clash]\n",
            "listed twice with different contents",
        ),
        (
            "[service.getty]\nuser = \"conao\"\n[service.same]\n",
            "\"none\" is not a parameter of this service",
        ),
        ("[service.absent]\n", "service.absent: read"),
    ] {
        let error = system_error_with(
            &format!("{SYSTEM_HEAD}{body}"),
            &[("getty", getty), ("clash", clash), ("same", same)],
        );
        assert!(error.contains(expected), "{body:?}: {error}");
    }
}

#[test]
fn garbage_collection_keeps_what_generations_list() {
    let root = tempfile::tempdir().expect("temporary directory");
    let store = root.path().join("data/store");
    let profiles = root.path().join("profiles");
    for id in ["aaaa", "bbbb", "ssss", "tttt"] {
        std::fs::create_dir_all(store.join(id).join("manifest")).expect("create");
    }
    std::fs::write(
        store.join("ssss/manifest/closure"),
        "/vug/store/ssss\n/vug/store/aaaa\n",
    )
    .expect("write");
    std::fs::write(
        store.join("tttt/manifest/closure"),
        "/vug/store/tttt\n/vug/store/bbbb\n",
    )
    .expect("write");
    std::fs::create_dir(&profiles).expect("create");
    std::os::unix::fs::symlink("/vug/store/tttt", profiles.join("system-1")).expect("symlink");
    std::os::unix::fs::symlink("/vug/store/ssss", profiles.join("system-2")).expect("symlink");
    std::os::unix::fs::symlink("system-2", profiles.join("system")).expect("symlink");
    let data = root.path().join("data");
    let args = |extra: &[&str]| {
        let mut words = vec![
            "store",
            "collect-garbage",
            "--data-dir",
            data.to_str().expect("path"),
            "--profiles",
            profiles.to_str().expect("path"),
        ];
        words.extend(extra);
        vug(&words)
    };

    let dry = args(&["--dry-run"]);
    assert_eq!(dry.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&dry.stdout).expect("JSON");
    assert_eq!(value["Removed"].as_array().map(Vec::len), Some(0));
    assert!(store.join("bbbb").exists());

    let deleted = vug(&[
        "system",
        "delete-generations",
        "--keep",
        "1",
        "--profiles",
        profiles.to_str().expect("path"),
    ]);
    assert_eq!(deleted.status.code(), Some(0), "{deleted:?}");
    assert!(!profiles.join("system-1").exists());
    assert!(profiles.join("system-2").symlink_metadata().is_ok());

    assert_eq!(args(&[]).status.code(), Some(0));
    assert!(store.join("ssss").exists() && store.join("aaaa").exists());
    assert!(!store.join("tttt").exists() && !store.join("bbbb").exists());

    std::fs::remove_file(profiles.join("system-2")).expect("remove");
    assert_ne!(args(&[]).status.code(), Some(0));
    assert!(store.join("aaaa").exists());
}

#[test]
fn signatures_verify_only_with_the_right_key_and_message() {
    let (secret, public) = vug::sign::generate().expect("key pair");
    let (_, other) = vug::sign::generate().expect("key pair");
    let signature = vug::sign::sign(&secret, "key id").expect("sign");
    assert!(vug::sign::verify(&public, "key id", &signature));
    assert!(!vug::sign::verify(&public, "key other", &signature));
    assert!(!vug::sign::verify(&other, "key id", &signature));
    assert!(!vug::sign::verify(&public, "key id", ""));
    assert!(vug::sign::sign("short", "key id").is_err());
}
