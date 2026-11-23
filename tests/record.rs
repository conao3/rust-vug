use std::path::{Path, PathBuf};
use std::process::Output;

fn vug(args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_vug"))
        .args(args)
        .output()
        .expect("run vug")
}

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("vug-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SHA: &str = "5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab";

fn package_toml(name: &str) -> String {
    format!(
        "name = \"{name}\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n"
    )
}

/// A canonical version record with `version` replaced.
fn version_toml(version: &str) -> String {
    version_toml_with(version, "")
}

fn version_toml_with(version: &str, extra_top: &str) -> String {
    format!(
        "version = \"{version}\"\n{extra_top}\n[source]\nurl = \"https://example.org/dist/x-{version}.tar.gz\"\nsha256 = \"{SHA}\"\n\n[build]\nkind = \"autotools\"\n\n[dependencies]\nbuild = []\nrun = []\n\n[check]\nenabled = true\nskip = []\n"
    )
}

/// A package directory `name` holding `package.toml` and `versions/`.
fn package_dir(tmp: &TmpDir, name: &str) -> PathBuf {
    let shard = if name.chars().count() == 1 {
        format!("{name}_")
    } else {
        name.chars().take(2).collect()
    };
    let dir = tmp.path().join("packages").join(shard).join(name);
    std::fs::create_dir_all(dir.join("versions")).unwrap();
    std::fs::write(dir.join("package.toml"), package_toml(name)).unwrap();
    dir
}

fn check_record(file: &Path) -> Output {
    vug(&["record", "check-record", "--file", file.to_str().unwrap()])
}

fn format_record(file: &Path) -> Output {
    vug(&["record", "format-record", "--file", file.to_str().unwrap()])
}

fn check_tree(root: &Path) -> Output {
    vug(&["record", "check-tree", "--root", root.to_str().unwrap()])
}

/// The first error message of a failing JSON report.
fn error_messages(output: &Output) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    value["Errors"]
        .as_array()
        .expect("Errors key")
        .iter()
        .map(|e| e["Message"].as_str().unwrap().to_owned())
        .collect()
}

fn assert_invalid(output: &Output, key: &str) {
    assert_eq!(output.status.code(), Some(254), "key {key}");
    let messages = error_messages(output);
    assert!(
        messages.iter().any(|m| m.contains(key)),
        "expected an error naming {key}, got {messages:?}"
    );
}

// -- package-record rules ---------------------------------------------------

#[test]
fn package_name_missing() {
    let tmp = TmpDir::new("name-missing");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "license = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "name");
}

#[test]
fn package_name_invalid() {
    let tmp = TmpDir::new("name-invalid");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(dir.join("package.toml"), package_toml("Hello")).unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "name");
}

#[test]
fn package_name_dir_mismatch() {
    let tmp = TmpDir::new("name-dir");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(dir.join("package.toml"), package_toml("other")).unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "name");
}

#[test]
fn package_license_missing() {
    let tmp = TmpDir::new("license");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "license");
}

#[test]
fn package_sources_missing() {
    let tmp = TmpDir::new("sources-missing");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "upstream.sources");
}

#[test]
fn package_sources_empty() {
    let tmp = TmpDir::new("sources-empty");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = []\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "upstream.sources");
}

#[test]
fn package_sources_http() {
    let tmp = TmpDir::new("sources-http");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"http://example.org/dist/\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "upstream.sources");
}

#[test]
fn package_sources_no_slash() {
    let tmp = TmpDir::new("sources-noslash");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "upstream.sources");
}

#[test]
fn package_sources_duplicate() {
    let tmp = TmpDir::new("sources-dup");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/a/\",\n  \"https://example.org/a/\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "upstream.sources");
}

#[test]
fn package_unknown_key() {
    let tmp = TmpDir::new("unknown-key");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\nextra = 1\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "extra");
}

#[test]
fn package_unknown_table() {
    let tmp = TmpDir::new("unknown-table");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n\n[extra]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "extra");
}

#[test]
fn package_name_wrong_type() {
    let tmp = TmpDir::new("name-type");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = 5\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n",
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "name");
}

#[test]
fn package_canonical_ok() {
    let tmp = TmpDir::new("pkg-ok");
    let dir = package_dir(&tmp, "hello");
    let output = check_record(&dir.join("package.toml"));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(error_messages(&output), Vec::<String>::new());
}

// -- version-record rules ---------------------------------------------------

#[test]
fn version_field_mismatch() {
    let tmp = TmpDir::new("version-mismatch");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, version_toml("2.0")).unwrap();
    assert_invalid(&check_record(&v), "version");
}

#[test]
fn version_field_invalid() {
    let tmp = TmpDir::new("version-invalid");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, version_toml("a@b")).unwrap();
    assert_invalid(&check_record(&v), "version");
}

#[test]
fn version_url_not_https() {
    let tmp = TmpDir::new("url-http");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("https://example.org", "http://example.org"),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "source.url");
}

#[test]
fn version_url_no_prefix() {
    let tmp = TmpDir::new("url-prefix");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("https://example.org", "https://evil.example.org"),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "source.url");
}

#[test]
fn version_sha256_bad() {
    let tmp = TmpDir::new("sha");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, version_toml("1.0").replace(SHA, "deadbeef")).unwrap();
    assert_invalid(&check_record(&v), "source.sha256");
}

#[test]
fn version_sha256_uppercase() {
    let tmp = TmpDir::new("sha-upper");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, version_toml("1.0").replace(SHA, &SHA.to_uppercase())).unwrap();
    assert_invalid(&check_record(&v), "source.sha256");
}

#[test]
fn version_strip_components_type() {
    let tmp = TmpDir::new("strip-type");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("sha256", "strip-components = \"x\"\nsha256"),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "strip-components");
}

#[test]
fn version_patches_bad_name() {
    let tmp = TmpDir::new("patch-name");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml_with("1.0", "patches = [\n  \"2.0-other.patch\",\n]\n"),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "patches");
}

#[test]
fn version_build_kind_bad() {
    let tmp = TmpDir::new("kind");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("kind = \"autotools\"", "kind = \"scons\""),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.kind");
}

#[test]
fn version_build_unknown_option() {
    let tmp = TmpDir::new("build-opt");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "[build]\nkind = \"autotools\"",
            "[build]\nkind = \"autotools\"\ninstall = []",
        ),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.install");
}

#[test]
fn version_prebuilt_install_missing() {
    let tmp = TmpDir::new("prebuilt-missing");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("kind = \"autotools\"", "kind = \"prebuilt\""),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.install");
}

#[test]
fn version_prebuilt_install_absolute() {
    let tmp = TmpDir::new("install-abs");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "kind = \"autotools\"",
            "kind = \"prebuilt\"\ninstall = [\n  { from = \"/x\", to = \"x\" },\n]",
        ),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.install[0].from");
}

#[test]
fn version_prebuilt_install_dotdot() {
    let tmp = TmpDir::new("install-dotdot");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "kind = \"autotools\"",
            "kind = \"prebuilt\"\ninstall = [\n  { from = \"x\", to = \"../x\" },\n]",
        ),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.install[0].to");
}

#[test]
fn version_dependencies_missing() {
    let tmp = TmpDir::new("deps");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("[dependencies]\nbuild = []\nrun = []\n\n", ""),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "dependencies");
}

#[test]
fn version_dependencies_duplicate() {
    let tmp = TmpDir::new("deps-dup");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace("build = []", "build = [\n  \"make\",\n  \"make\",\n]"),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "dependencies.build");
}

#[test]
fn version_check_enabled_missing() {
    let tmp = TmpDir::new("check");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, version_toml("1.0").replace("enabled = true\n", "")).unwrap();
    assert_invalid(&check_record(&v), "check.enabled");
}

#[test]
fn skip_requires_a_supported_build_kind() {
    let tmp = TmpDir::new("check-skip");
    let dir = package_dir(&tmp, "hello");
    let path = dir.join("versions/1.0.toml");
    let record = version_toml("1.0").replace("skip = []", "skip = [\n  \"test-float-h\",\n]");
    std::fs::write(&path, &record).unwrap();
    assert!(check_record(&path).status.success());
    std::fs::write(&path, record.replace("enabled = true", "enabled = false")).unwrap();
    assert!(check_record(&path).status.success());
    for invalid in [
        record.replace("kind = \"autotools\"", "kind = \"make\""),
        record.replace("test-float-h", "test-%"),
    ] {
        std::fs::write(&path, invalid).unwrap();
        assert_invalid(&check_record(&path), "check.skip");
    }
}

#[test]
fn version_unknown_table() {
    let tmp = TmpDir::new("v-table");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, format!("{}\n[extra]\n", version_toml("1.0"))).unwrap();
    assert_invalid(&check_record(&v), "extra");
}

#[test]
fn version_not_toml_name() {
    let tmp = TmpDir::new("not-toml");
    let file = tmp.write("hello/record.txt", "version = \"1.0\"\n");
    assert_invalid(&check_record(&file), "file name");
}

#[test]
fn version_missing_file() {
    let tmp = TmpDir::new("missing");
    let file = tmp.path().join("nope.toml");
    assert_eq!(check_record(&file).status.code(), Some(254));
}

#[test]
fn version_canonical_ok() {
    let tmp = TmpDir::new("v-ok");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(&v, version_toml("1.0")).unwrap();
    let output = check_record(&v);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

// -- canonicality / format --------------------------------------------------

#[test]
fn unsorted_array_is_not_canonical() {
    let tmp = TmpDir::new("unsorted");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        "name = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/b/\",\n  \"https://example.org/a/\",\n]\n",
    )
    .unwrap();
    let output = check_record(&dir.join("package.toml"));
    assert_eq!(output.status.code(), Some(254));
    let output = format_record(&dir.join("package.toml"));
    assert_eq!(output.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["Changed"], true);
    assert!(
        value["Record"]
            .as_str()
            .unwrap()
            .find("https://example.org/a/")
            .unwrap()
            < value["Record"]
                .as_str()
                .unwrap()
                .find("https://example.org/b/")
                .unwrap()
    );
}

#[test]
fn format_write_rewrites() {
    let tmp = TmpDir::new("write");
    let dir = package_dir(&tmp, "hello");
    let file = dir.join("package.toml");
    std::fs::write(
        &file,
        "# comment\nname = \"hello\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"https://example.org/dist/\",\n]\n",
    )
    .unwrap();
    let output = vug(&[
        "record",
        "format-record",
        "--file",
        file.to_str().unwrap(),
        "--write",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        package_toml("hello")
    );
    assert_eq!(check_record(&file).status.code(), Some(0));
}

#[test]
fn comments_make_file_noncanonical() {
    let tmp = TmpDir::new("comments");
    let dir = package_dir(&tmp, "hello");
    std::fs::write(
        dir.join("package.toml"),
        format!("# hello\n{}", package_toml("hello")),
    )
    .unwrap();
    assert_invalid(&check_record(&dir.join("package.toml")), "canonical");
}

// -- fixture trees ----------------------------------------------------------

#[test]
fn check_tree_valid_fixture() {
    let output = check_tree(Path::new("tests/fixtures/tree-valid"));
    assert_eq!(output.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["Packages"], 3);
    assert_eq!(value["Versions"], 4);
    assert_eq!(value["Errors"], serde_json::json!([]));
}

#[test]
fn check_tree_error_fixture() {
    let output = check_tree(Path::new("tests/fixtures/tree-errors"));
    assert_eq!(output.status.code(), Some(254));
    let messages = error_messages(&output).join("\n");
    for expected in [
        "name: expected \"badname\"",
        "version: expected \"1.0\"",
        "source.url: expected a URL starting with one of upstream.sources",
        "patches: file \"1.0-missing.patch\" not found",
        "not in canonical form",
        "unexpected entry \"notes.txt\"",
    ] {
        assert!(
            messages.contains(expected),
            "missing {expected}\n{messages}"
        );
    }
}

#[test]
fn check_tree_text_output() {
    let output = vug(&[
        "--output",
        "text",
        "record",
        "check-tree",
        "--root",
        "tests/fixtures/tree-errors",
    ]);
    assert_eq!(output.status.code(), Some(254));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Packages: 6\n"), "{text}");
    assert!(text.contains("Versions: 3\n"), "{text}");
    for line in text.lines().skip(2) {
        assert!(line.contains(": "), "PATH: MESSAGE line, got {line}");
    }
}

#[test]
fn check_tree_missing_packages_dir() {
    let tmp = TmpDir::new("no-packages");
    let output = check_tree(tmp.path());
    assert_eq!(output.status.code(), Some(254));
    assert_invalid(&output, "packages");
}

// -- help and usage ---------------------------------------------------------

#[test]
fn record_help_levels() {
    for args in [
        vec!["record", "help"],
        vec!["record", "format-record", "help"],
        vec!["record", "check-record", "help"],
        vec!["record", "check-tree", "help"],
    ] {
        let output = vug(&args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(
            String::from_utf8(output.stdout).unwrap().contains("Usage"),
            "{args:?}"
        );
    }
}

#[test]
fn record_usage_errors() {
    for args in [
        vec!["record"],
        vec!["record", "format-record"],
        vec!["record", "check-record"],
        vec!["record", "check-tree"],
        vec!["record", "bogus"],
    ] {
        let output = vug(&args);
        assert_eq!(output.status.code(), Some(252), "{args:?}");
    }
}

#[test]
fn version_bootstrap_make_kind() {
    let tmp = TmpDir::new("bootstrap-make");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "kind = \"autotools\"",
            "kind = \"bootstrap-make\"\nconfigure-flags = [\n  \"--disable-nls\",\n]",
        ),
    )
    .unwrap();
    assert_eq!(check_record(&v).status.code(), Some(0));
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "kind = \"autotools\"",
            "kind = \"bootstrap-make\"\nmake-flags = []",
        ),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.");
}

#[test]
fn version_linux_kind() {
    let tmp = TmpDir::new("linux-kind");
    let dir = package_dir(&tmp, "hello");
    let v = dir.join("versions/1.0.toml");
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "kind = \"autotools\"",
            "kind = \"linux\"\ndefconfig = \"tinyconfig\"\nconfig = [\n  \"64BIT=y\",\n  \"KERNEL_XZ=n\",\n]",
        ),
    )
    .unwrap();
    assert_eq!(check_record(&v).status.code(), Some(0));
    std::fs::write(
        &v,
        version_toml("1.0").replace(
            "kind = \"autotools\"",
            "kind = \"linux\"\ndefconfig = \"tinyconfig\"\nconfig = [\n  \"64bit\",\n]",
        ),
    )
    .unwrap();
    assert_invalid(&check_record(&v), "build.config");
}

fn with_outputs(record: &str, outputs: &str) -> String {
    record.replace("[check]", &format!("{outputs}\n[check]"))
}

#[test]
fn format_outputs_in_canonical_order() {
    let tmp = TmpDir::new("format-outputs");
    let record = version_toml("1").replace("build = []", "build = [\"llvm:dev\", \"llvm:lib\"]");
    let input = with_outputs(
        &record,
        r#"[outputs.out]
[outputs."dev.sdk"]
run = ["zlib", ":lib"]
paths = ["include/**", "bin/header-tool"]

[outputs.lib]
paths = ["lib/*.so.*"]
"#,
    );
    let file = package_dir(&tmp, "consumer").join("versions/1.toml");
    std::fs::write(&file, input).unwrap();
    let result = format_record(&file);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let canonical = result["Record"].as_str().unwrap();
    assert!(canonical.contains(
        "[outputs.\"dev.sdk\"]\npaths = [\n  \"bin/header-tool\",\n  \"include/**\",\n]\nrun = [\n  \":lib\",\n  \"zlib\",\n]\n\n[outputs.lib]\npaths = [\n  \"lib/*.so.*\",\n]\nrun = []\n\n[outputs.out]\nrun = []"
    ));
    std::fs::write(&file, canonical).unwrap();
    assert!(check_record(&file).status.success());
}

#[test]
fn reject_invalid_output_declarations() {
    let tmp = TmpDir::new("invalid-outputs");
    let file = package_dir(&tmp, "consumer").join("versions/1.toml");
    for (declarations, diagnostic) in [
        ("[outputs.lib]\nrun = []\n", "outputs.out"),
        ("[outputs.out]\nrun = [\":lib\"]\n", "unknown output"),
        (
            "[outputs.out]\nrun = [\":lib\"]\n[outputs.lib]\nrun = [\":out\"]\n",
            "dependency cycle",
        ),
        ("[outputs.out]\nrun = [\":out\"]\n", "dependency cycle"),
        (
            "[outputs.out]\npaths = [\"../lib\"]\n",
            "inside the staging tree",
        ),
        (
            "[outputs.out]\npaths = [\"lib/**/a?.so\"]\n[outputs.dev]\npaths = [\"lib/**/a*.so\"]\n",
            "overlaps",
        ),
        (
            "[outputs.out]\npaths = [\"bin/tool\"]\n[outputs.dev]\npaths = [\"bin/tool\"]\n",
            "overlaps",
        ),
    ] {
        std::fs::write(&file, with_outputs(&version_toml("1"), declarations)).unwrap();
        assert_invalid(&format_record(&file), diagnostic);
    }
    for (build, run, output, diagnostic) in [
        ("[\":lib\"]", "[]", "", "invalid dependency"),
        ("[\"empty:\"]", "[]", "", "invalid dependency"),
        ("[\"foo:out:lib\"]", "[]", "", "invalid dependency"),
        ("[\"foo\", \"foo:out\"]", "[]", "", "select the same output"),
        (
            "[]",
            "[\"foo\"]",
            "[outputs.out]\nrun = []\n",
            "expected an empty array",
        ),
    ] {
        let record = version_toml("1").replace(
            "build = []\nrun = []",
            &format!("build = {build}\nrun = {run}"),
        );
        std::fs::write(&file, with_outputs(&record, output)).unwrap();
        assert_invalid(&format_record(&file), diagnostic);
    }
}

#[test]
fn check_tree_resolves_outputs_on_current_dependency_versions() {
    let tmp = TmpDir::new("check-output-tree");
    let consumer_dir = package_dir(&tmp, "consumer");
    let library_dir = package_dir(&tmp, "library");
    let consumer = consumer_dir.join("versions/1.toml");
    let library_v2 = library_dir.join("versions/2.toml");
    std::fs::write(
        &consumer,
        version_toml("1").replace(
            "build = []\nrun = []",
            "build = [\n  \"library:dev\",\n]\nrun = [\n  \"library:lib\",\n]\nexpose = [\n  \"library:dev\",\n]",
        ),
    )
    .unwrap();
    let library_outputs = "[outputs.dev]\npaths = [\n  \"include/**\",\n]\nrun = [\n  \":lib\",\n]\n\n[outputs.lib]\npaths = [\n  \"lib/*.so.*\",\n]\nrun = []\n\n[outputs.out]\nrun = []\n";
    std::fs::write(
        library_dir.join("versions/1.toml"),
        with_outputs(&version_toml("1"), library_outputs),
    )
    .unwrap();
    std::fs::write(&library_v2, version_toml("2")).unwrap();
    assert_invalid(&check_tree(tmp.path()), "unknown output \"library:dev\"");
    assert_invalid(&check_tree(tmp.path()), "unknown output \"library:lib\"");
    assert!(check_record(&consumer).status.success());

    let current_library = with_outputs(&version_toml("2"), library_outputs);
    std::fs::write(&library_v2, &current_library).unwrap();
    let result = check_tree(tmp.path());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );

    std::fs::write(
        &library_v2,
        current_library.replace("build = []", "build = [\n  \"consumer\",\n]"),
    )
    .unwrap();
    assert_invalid(&check_tree(tmp.path()), "consumer -> library -> consumer");
}
