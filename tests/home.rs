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

fn home_json(args: &[&str]) -> serde_json::Value {
    let output = home(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn outputs_are_reproducible_and_gc_keeps_the_current_generation() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let build = |state: &str, data: &str| {
        home_json(&[
            "build-home",
            "--root",
            root.to_str().unwrap(),
            "--name",
            "sample",
            "--state-dir",
            root.join(state).to_str().unwrap(),
            "--data-dir",
            root.join(data).to_str().unwrap(),
        ])
    };
    let collect = |command: &str, extra: &[&str]| {
        let state = root.join("state");
        let data = root.join("data");
        let mut args = vec![
            command,
            "--state-dir",
            state.to_str().unwrap(),
            "--data-dir",
            data.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        home_json(&args)
    };

    write(
        root,
        "homes/sample.toml",
        r#"
name = "sample"
[environment]
LITERAL = "$HOME"
[[file]]
path = ".config/example"
content = "/vug/store/00000000000000000000000000000000"
[[unit]]
name = "example.service"
enable = ["default.target"]
secrets = ["key"]
content = "[Service]\nType=oneshot\n"
[[secret]]
name = "key"
"#,
    );
    let first = build("state", "data");
    assert_eq!(first, build("state", "data"));
    assert_eq!(first["Id"], build("other-state", "other-data")["Id"]);
    let path = Path::new(first["Path"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(path.join("files/.config/example")).unwrap(),
        "/vug/store/00000000000000000000000000000000"
    );
    assert!(
        std::fs::metadata(path.join("files/.config/example"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(
        std::fs::read_link(path.join("units/default.target.wants/example.service")).unwrap(),
        Path::new("../example.service")
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("manifest/home.json")).unwrap()).unwrap();
    assert_eq!(manifest["Secrets"], serde_json::json!(["key"]));
    assert_eq!(
        manifest["Units"]["example.service"]["Enable"],
        serde_json::json!(["default.target"])
    );
    assert!(!root.join("state/current").exists());

    std::os::unix::fs::symlink("generations/1", root.join("state/current")).unwrap();
    write(
        root,
        "homes/sample.toml",
        "name = 'sample'\n[environment]\nLITERAL = 'second'\n",
    );
    assert_eq!(build("state", "data")["Generation"], 2);
    write(
        root,
        "homes/sample.toml",
        "name = 'sample'\n[environment]\nLITERAL = 'third'\n",
    );
    assert_eq!(build("state", "data")["Generation"], 3);

    assert_eq!(
        collect("delete-generations", &["--keep", "1"])["Removed"],
        serde_json::json!([2])
    );
    let dry = collect("collect-garbage", &["--dry-run"]);
    assert_eq!(dry["Kept"], 2);
    let stale = dry["Removed"][0].as_str().unwrap();
    assert!(root.join("data/store").join(stale).is_dir());
    assert_eq!(collect("collect-garbage", &[])["Removed"], dry["Removed"]);
    assert!(!root.join("data/store").join(stale).exists());
    assert_eq!(
        std::fs::canonicalize(root.join("state/current")).unwrap(),
        path
    );
}

struct HomeProfile {
    _dir: TempDir,
    home: std::path::PathBuf,
    repo: std::path::PathBuf,
    state: std::path::PathBuf,
}

impl HomeProfile {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        let repo = dir.path().join("repo");
        let state = home.join(".local/state/vug/home");
        std::fs::create_dir(&home).unwrap();
        Self {
            _dir: dir,
            home,
            repo,
            state,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vug"));
        command
            .env("HOME", &self.home)
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_DATA_HOME")
            .arg("home")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> serde_json::Value {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn error(&self, args: &[&str], message: &str) {
        let output = self.command(args).output().unwrap();
        assert_eq!(output.status.code(), Some(254));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
    }

    fn build(&self, files: &[(&str, &str)], unit: bool) -> u64 {
        let mut text = "name = 'example'\n".to_owned();
        for (path, contents) in files {
            text.push_str(&format!(
                "[[file]]\npath = '{path}'\ncontent = '{contents}'\n"
            ));
        }
        if unit {
            text.push_str(
                "[[unit]]\nname = 'example.service'\nenable = ['default.target']\ncontent = '[Service]'\n",
            );
        }
        write(&self.repo, "homes/example.toml", &text);
        self.run(&[
            "build-home",
            "--root",
            self.repo.to_str().unwrap(),
            "--name",
            "example",
        ])["Generation"]
            .as_u64()
            .unwrap()
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.home.join(path)).unwrap()
    }

    fn assert_current(&self, number: u64) {
        assert_eq!(
            std::fs::read_link(self.state.join("current")).unwrap(),
            Path::new("generations").join(number.to_string())
        );
    }
}

#[test]
fn switching_and_rollback_apply_files_and_unit_links() {
    let profile = HomeProfile::new();
    assert_eq!(
        profile.build(&[("config/common", "first"), ("old", "old")], true),
        1
    );
    assert_eq!(
        profile.build(&[("config/common", "second"), ("new", "new")], false),
        2
    );
    write(&profile.home, "config/local", "keep");
    profile.error(&["rollback-home"], "no active home generation");

    assert_eq!(
        profile.run(&["switch-home", "--generation", "1"])["Generation"],
        1
    );
    profile.assert_current(1);
    let common = profile.home.join("config/common");
    let link = std::fs::read_link(&common).unwrap();
    let inode = std::os::unix::fs::MetadataExt::ino(&std::fs::symlink_metadata(&common).unwrap());
    assert_eq!(link, profile.state.join("current/files/config/common"));
    assert_eq!(profile.read("config/common"), "first");
    assert_eq!(profile.read("old"), "old");
    let unit = ".config/systemd/user/example.service";
    let wants = ".config/systemd/user/default.target.wants/example.service";
    assert_eq!(profile.read(unit), "[Service]");
    assert_eq!(profile.read(wants), "[Service]");

    assert_eq!(
        profile.run(&["switch-home", "--generation", "2"])["Generation"],
        2
    );
    profile.assert_current(2);
    assert_eq!(profile.read("config/common"), "second");
    assert_eq!(std::fs::read_link(&common).unwrap(), link);
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::symlink_metadata(&common).unwrap()),
        inode
    );
    for removed in ["old", unit, wants] {
        assert!(!profile.home.join(removed).is_symlink());
    }
    assert_eq!(profile.read("new"), "new");

    assert_eq!(profile.run(&["rollback-home"])["Generation"], 1);
    profile.assert_current(1);
    assert_eq!(profile.read("config/common"), "first");
    assert_eq!(profile.read("config/local"), "keep");
    assert_eq!(profile.read(wants), "[Service]");
    assert!(!profile.home.join("new").is_symlink());
    assert!(!profile.state.join("activation-pending.json").exists());
    profile.run(&["switch-home", "--generation", "1"]);
    profile.error(&["rollback-home"], "no earlier retained home generation");
}

#[test]
fn preflight_preserves_unmanaged_entries_and_replaced_links() {
    use std::os::unix::fs::symlink;
    let profile = HomeProfile::new();
    profile.build(&[("active", "first")], false);
    profile.build(
        &[("aaa", "new"), ("active", "second"), ("nested/item", "new")],
        false,
    );
    let command = ["switch-home", "--generation", "2"];
    profile.run(&["switch-home", "--generation", "1"]);
    write(&profile.home, "nested/item", "manual");
    profile.error(&command, "home path collision");
    profile.assert_current(1);
    assert_eq!(profile.read("nested/item"), "manual");
    assert!(!profile.home.join("aaa").is_symlink());
    assert!(!profile.state.join("activation-pending.json").exists());

    let nested = profile.home.join("nested");
    std::fs::rename(&nested, profile.repo.join("outside")).unwrap();
    symlink(profile.repo.join("outside"), &nested).unwrap();
    profile.error(&command, "home path collision");
    assert_eq!(
        std::fs::read_to_string(profile.repo.join("outside/item")).unwrap(),
        "manual"
    );
    std::fs::remove_file(&nested).unwrap();
    std::fs::create_dir(&nested).unwrap();
    symlink(profile.repo.join("outside/item"), nested.join("item")).unwrap();
    profile.error(&command, "home path collision");
    std::fs::remove_file(nested.join("item")).unwrap();

    let active = profile.home.join("active");
    std::fs::remove_file(&active).unwrap();
    let altered = profile.state.join("current/files/./active");
    symlink(&altered, &active).unwrap();
    profile.error(&command, "home path collision");
    assert_eq!(
        std::fs::read_link(active).unwrap().as_os_str(),
        altered.as_os_str()
    );
    assert_eq!(profile.read("active"), "first");
    profile.assert_current(1);
    assert!(!profile.home.join("aaa").is_symlink());
    let current = profile.state.join("current");
    std::fs::remove_file(&current).unwrap();
    symlink("generations/2", &current).unwrap();
    profile.error(&command, "differs from the last recorded activation");
}

#[test]
fn interrupted_switch_keeps_generation_roots_and_retries_recovery() {
    use std::os::unix::fs::symlink;
    let profile = HomeProfile::new();
    profile.build(&[("old", "first")], false);
    profile.build(&[("new", "second")], false);
    profile.build(&[("latest", "third")], false);
    profile.run(&["switch-home", "--generation", "1"]);

    let pending = profile.state.join("activation-pending.json");
    let active: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profile.state.join("activation-home.json")).unwrap())
            .unwrap();
    std::fs::write(
        &pending,
        serde_json::to_vec(&serde_json::json!({"home": active["home"], "from": 1, "to": 2}))
            .unwrap(),
    )
    .unwrap();
    std::fs::remove_file(profile.home.join("old")).unwrap();
    let current = profile.state.join("current");
    std::fs::remove_file(&current).unwrap();
    symlink("generations/2", &current).unwrap();
    write(&profile.home, "new", "changed by hand after interruption");
    let retry = ["switch-home", "--generation", "1"];
    profile.error(&retry, "could not recover unfinished home activation");
    assert_eq!(profile.read("new"), "changed by hand after interruption");
    assert!(pending.is_file());

    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(profile.state.join("lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut gc = profile
        .command(&["collect-garbage"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(gc.try_wait().unwrap().is_none());
    lock.unlock().unwrap();
    let gc = gc.wait_with_output().unwrap();
    assert!(gc.status.success(), "{gc:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&gc.stdout).unwrap()["Kept"],
        3
    );
    assert_eq!(
        profile.run(&["delete-generations", "--keep", "1"])["Removed"],
        serde_json::json!([])
    );

    std::fs::remove_file(profile.home.join("new")).unwrap();
    symlink(
        profile.state.join("current/files/new"),
        profile.home.join("new"),
    )
    .unwrap();
    profile.run(&retry);
    profile.assert_current(1);
    assert_eq!(profile.read("old"), "first");
    assert!(!profile.home.join("new").is_symlink());
    assert!(!pending.exists());
    assert_eq!(
        profile.run(&["delete-generations", "--keep", "1"])["Removed"],
        serde_json::json!([2])
    );
    assert_eq!(profile.run(&["collect-garbage"])["Kept"], 2);
    profile.run(&["switch-home", "--generation", "3"]);
    assert_eq!(profile.read("latest"), "third");
    std::fs::write(
        &pending,
        serde_json::to_vec(&serde_json::json!({"home": active["home"], "from": 1, "to": 3}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        profile.run(&["switch-home", "--generation", "3"])["Generation"],
        3
    );
    assert_eq!(profile.read("latest"), "third");
    assert!(!pending.exists());
}
