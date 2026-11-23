use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn home(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vug"))
        .arg("home")
        .args(args)
        .output()
        .unwrap()
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn compose(root: &Path, name: &str) -> Output {
    home(&[
        "compose-home",
        "--root",
        root.to_str().unwrap(),
        "--name",
        name,
    ])
}

fn record(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    json["Record"].as_str().unwrap().to_owned()
}

fn assert_error(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(254));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = json["Errors"][0]["Message"].as_str().unwrap();
    assert!(message.contains(expected), "{message}");
}

#[test]
fn compose_is_deterministic_and_deduplicates_identical_definitions() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let local = r#"
packages = ["world", "hello", "hello"]
[environment]
EDITOR = "vi"
LITERAL = "$HOME"
[[file]]
path = ".config/sample"
content = "line\n"
[[file]]
path = ".local/bin/sample"
package = "tools"
source = "bin/sample"
[[unit]]
name = "sample.service"
enable = ["default.target", "graphical-session.target", "default.target"]
secrets = ["sample-key", "sample-key"]
content = "ExecStart={pkg:tools}/bin/sample"
[[secret]]
name = "sample-key"
"#;
    let fragment = r#"
name = "alpha"
packages = ["tools", "world"]
[environment]
EDITOR = "vi"
[[file]]
path = ".config/sample"
content = "line\n"
[[unit]]
name = "sample.service"
enable = ["graphical-session.target", "default.target"]
secrets = ["sample-key"]
content = "ExecStart={pkg:tools}/bin/sample"
[[secret]]
name = "sample-key"
"#;
    write(root, "home-fragments/alpha.toml", fragment);
    write(root, "home-fragments/beta.toml", "name = 'beta'\n");
    write(
        root,
        "homes/desktop.toml",
        &format!("fragments = ['beta', 'alpha', 'alpha']\nname = 'desktop'\n{local}"),
    );
    let first = record(&compose(root, "desktop"));
    write(
        root,
        "homes/desktop.toml",
        &format!("name = 'desktop'\nfragments = ['alpha', 'beta']\n{local}"),
    );
    assert_eq!(first, record(&compose(root, "desktop")));

    let table: toml::Table = first.parse().unwrap();
    assert_eq!(table["name"].as_str(), Some("desktop"));
    assert!(!table.contains_key("fragments"));
    assert_eq!(
        table["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hello", "tools", "world"]
    );
    assert_eq!(table["file"].as_array().unwrap().len(), 2);
    assert_eq!(table["unit"].as_array().unwrap().len(), 1);
    assert_eq!(table["secret"].as_array().unwrap().len(), 1);
    assert_eq!(table["environment"]["LITERAL"].as_str(), Some("$HOME"));
}

#[test]
fn format_home_and_fragment_are_idempotent() {
    let dir = TempDir::new().unwrap();
    for (command, text) in [
        (
            "format-home",
            "fragments = ['shell', 'shell']\nname = 'sample'\npackages = ['b', 'a']\n",
        ),
        (
            "format-fragment",
            "name='sample'\n[[file]]\npath='a'\ncontent='hello'\n[[file]]\npath='a'\ncontent='hello'\n",
        ),
    ] {
        let path = dir.path().join("sample.toml");
        std::fs::write(&path, text).unwrap();
        let args = [command, "--file", path.to_str().unwrap(), "--write"];
        let canonical = record(&home(&args));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), canonical);
        let again = home(&args);
        assert_eq!(record(&again), canonical);
        let json: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
        assert_eq!(json["Changed"], false);
    }
}

#[test]
fn composition_rejects_conflicting_definitions() {
    for (local, fragment, expected) in [
        (
            "[environment]\nEDITOR = 'vi'",
            "[environment]\nEDITOR = 'vim'",
            "environment: conflicting definitions",
        ),
        (
            "[[file]]\npath = 'config'\ncontent = 'a'",
            "[[file]]\npath = 'config'\ncontent = 'b'",
            "file.path: conflicting definitions",
        ),
        (
            "[[file]]\npath = 'config/item'\ncontent = 'a'\n[[file]]\npath = 'config-extra'\ncontent = 'a'",
            "[[file]]\npath = 'config'\ncontent = 'b'",
            "is inside listed path",
        ),
        (
            "[[unit]]\nname = 'test.service'\ncontent = 'a'",
            "[[unit]]\nname = 'test.service'\ncontent = 'b'",
            "unit.name: conflicting definitions",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            root,
            "homes/sample.toml",
            &format!("name = 'sample'\nfragments = ['test']\n{local}\n"),
        );
        write(
            root,
            "home-fragments/test.toml",
            &format!("name = 'test'\n{fragment}\n"),
        );
        assert_error(compose(root, "sample"), expected);
    }
}

#[test]
fn composition_rejects_invalid_records_and_references() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (text, expected) in [
        ("unknown = true", "unknown field"),
        ("[[secret]]\nname = 'test'\nvalue = 'x'", "unknown field"),
        (
            "[[file]]\npath = '.config/systemd/user/a'\ncontent = ''",
            "reserved path",
        ),
        (
            "[[file]]\npath = '.local/state'\ncontent = ''",
            "reserved path",
        ),
        (
            "[[file]]\npath = 'a/../b'\ncontent = ''",
            "normalized relative path",
        ),
        (
            "[[file]]\npath = '/a'\ncontent = ''",
            "normalized relative path",
        ),
        (
            "[[file]]\npath = 'a//b'\ncontent = ''",
            "normalized relative path",
        ),
        (
            "[[file]]\npath = 'a/./b'\ncontent = ''",
            "normalized relative path",
        ),
        (
            "[[file]]\npath = 'a'\npackage = 'missing'\nsource = 'b'",
            "not selected",
        ),
        (
            "[[unit]]\nname = 'x.service'\ncontent = ''\nsecrets = ['missing']",
            "not declared",
        ),
        ("fragments = ['../outside']", "invalid fragment name"),
        ("fragments = ['missing']", "cannot read"),
    ] {
        write(
            root,
            "homes/sample.toml",
            &format!("name = 'sample'\n{text}\n"),
        );
        assert_error(compose(root, "sample"), expected);
    }
    write(
        root,
        "homes/sample.toml",
        "name = 'sample'\nfragments = ['test']\n",
    );
    write(
        root,
        "home-fragments/test.toml",
        "name = 'test'\nfragments = []\n",
    );
    assert_error(compose(root, "sample"), "unknown key in a home fragment");
    write(root, "home-fragments/test.toml", "name = 'other'\n");
    assert_error(compose(root, "sample"), "matching the file name");
}
