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

#[test]
fn tree_sitter_grammar_record_round_trips() {
    let tmp = TmpDir::new("tree-sitter");
    let dir = package_dir(&tmp, "tree-sitter-tsx");
    let record = version_toml("0.23.2").replace(
        "kind = \"autotools\"",
        "kind = \"tree-sitter\"\npath = \"tsx\"\nlanguage = \"tsx\"",
    );
    let version = dir.join("versions/0.23.2.toml");
    std::fs::write(&version, &record).unwrap();
    assert!(check_record(&version).status.success());
    let formatted = format_record(&version);
    assert!(formatted.status.success());
    let formatted: serde_json::Value = serde_json::from_slice(&formatted.stdout).unwrap();
    assert_eq!(formatted["Record"].as_str().unwrap(), record);

    let generated = record.replace("language = \"tsx\"", "language = \"tsx\"\ngenerate = true");
    std::fs::write(&version, &generated).unwrap();
    let formatted = format_record(&version);
    assert!(formatted.status.success());
    let formatted: serde_json::Value = serde_json::from_slice(&formatted.stdout).unwrap();
    assert_eq!(formatted["Record"].as_str().unwrap(), generated);

    std::fs::write(
        &version,
        record.replace("language = \"tsx\"", "language = \"../tsx\""),
    )
    .unwrap();
    assert_invalid(&check_record(&version), "build.language");
    std::fs::write(
        &version,
        record.replace("path = \"tsx\"", "path = \"../tsx\""),
    )
    .unwrap();
    assert_invalid(&check_record(&version), "build.path");
}

const WHICH_NIX_RECORD: &str = r#"version = "2.25"

[build]
kind = "nix-output"
system = "x86_64-linux"
nixpkgs-rev = "2786a2a72088ecf32aea5b3ec52099e6a7cff78f"
attribute = "which"

[build.nix-outputs.out]
store-path = "/nix/store/jv5kdlkwx0l8a7jdpg21g934ff30s319-which-2.25"
nar-hash = "sha256:0sgl66vyp75kspr2qkkg5z54jqrfdqbmd2pww1aqvlvh6c26b6rg"
expose = [
  "bin/which",
]

[dependencies]
build = []
run = []

[check]
enabled = false
skip = []
"#;

#[test]
fn checks_nix_output_records_and_dependency_roles() {
    let tmp = TmpDir::new("nix-output-record");
    let which_dir = package_dir(&tmp, "which");
    let which = which_dir.join("versions/2.25.toml");
    std::fs::write(&which, WHICH_NIX_RECORD).unwrap();
    assert!(check_record(&which).status.success());
    assert!(check_tree(tmp.path()).status.success());

    for (old, new, field) in [
        ("x86_64-linux", "x86_64", "build.system"),
        (
            "2786a2a72088ecf32aea5b3ec52099e6a7cff78f",
            "master",
            "build.nixpkgs-rev",
        ),
        (
            "attribute = \"which\"",
            "attribute = \".which\"",
            "build.attribute",
        ),
        ("/nix/store/", "/tmp/", "store-path"),
        ("sha256:0sgl", "sha256:zsgl", "nar-hash"),
        ("bin/which", "lib/libc.so", "expose"),
        ("bin/which", "bin/../which", "expose"),
        ("bin/which", "bin/which\", \"bin/which/other", "expose"),
        ("enabled = false", "enabled = true", "check"),
    ] {
        std::fs::write(&which, WHICH_NIX_RECORD.replace(old, new)).unwrap();
        assert_invalid(&check_record(&which), field);
    }
    std::fs::write(
        &which,
        WHICH_NIX_RECORD.replace("bin/which", "share/doc/which\", \"bin/which"),
    )
    .unwrap();
    assert_invalid(&check_record(&which), "canonical");
    let formatted = format_record(&which);
    assert!(formatted.status.success());
    let stdout: serde_json::Value = serde_json::from_slice(&formatted.stdout).unwrap();
    let canonical = stdout["Record"].as_str().unwrap();
    assert!(canonical.contains("  \"bin/which\",\n  \"share/doc/which\","));
    std::fs::write(&which, WHICH_NIX_RECORD).unwrap();

    let user = package_dir(&tmp, "user").join("versions/1.toml");
    let native = version_toml("1");
    for field in ["build", "expose"] {
        let dependency = if field == "build" {
            native.replace("build = []", "build = [\n  \"which\",\n]")
        } else {
            native.replace("run = []", "run = []\nexpose = [\n  \"which:out\",\n]")
        };
        std::fs::write(&user, dependency).unwrap();
        assert_invalid(&check_tree(tmp.path()), &format!("dependencies.{field}"));
    }
    std::fs::write(
        &user,
        native.replace("run = []", "run = [\n  \"which\",\n]"),
    )
    .unwrap();
    assert!(check_tree(tmp.path()).status.success());
    std::fs::write(
        &which,
        WHICH_NIX_RECORD.replace("nix-outputs.out", "nix-outputs.bin"),
    )
    .unwrap();
    assert_invalid(&check_tree(tmp.path()), "unknown output");
}

#[test]
#[ignore = "requires access to cache.nixos.org"]
fn builds_and_runs_which_from_the_public_nix_cache() {
    let tmp = TmpDir::new("nix-output-which");
    let which = package_dir(&tmp, "which").join("versions/2.25.toml");
    std::fs::write(which, WHICH_NIX_RECORD).unwrap();
    tmp.write("seed.toml", "");
    let data = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let args = [
        "package",
        "build-package",
        "--root",
        tmp.path().to_str().unwrap(),
        "--name",
        "which",
        "--jobs",
        "2",
        "--data-dir",
        data.path().to_str().unwrap(),
        "--cache-dir",
        cache.path().to_str().unwrap(),
    ];
    let build = || {
        let output = vug(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let first = build();
    let again = build();
    assert_eq!(first["Built"], true);
    assert_eq!(again["Built"], false);
    assert_eq!(first["Path"], again["Path"]);
    let out = Path::new(first["HostPath"].as_str().unwrap());
    let link = std::fs::read_link(out.join("bin/which")).unwrap();
    assert_eq!(
        link,
        Path::new("/nix/store/jv5kdlkwx0l8a7jdpg21g934ff30s319-which-2.25/bin/which")
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join(".vug/nix-closure.json")).unwrap()).unwrap();
    assert_eq!(manifest["format"], 1);
    assert_eq!(manifest["output"], "out");
    assert_eq!(manifest["build-key"].as_str().unwrap().len(), 32);
    let paths = manifest["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 5);
    assert!(paths.iter().any(|entry| {
        entry["store-path"] == "/nix/store/jv5kdlkwx0l8a7jdpg21g934ff30s319-which-2.25"
            && entry["nar-hash"] == "sha256:0sgl66vyp75kspr2qkkg5z54jqrfdqbmd2pww1aqvlvh6c26b6rg"
    }));
    for entry in paths {
        let path = entry["store-path"].as_str().unwrap();
        let local = data
            .path()
            .join("nix/store")
            .join(Path::new(path).file_name().unwrap());
        assert!(std::fs::symlink_metadata(local).is_ok());
        for reference in entry["references"].as_array().unwrap() {
            assert!(paths.iter().any(|entry| &entry["store-path"] == reference));
        }
    }
    assert!(
        std::fs::metadata(
            data.path()
                .join("nix/store")
                .join(link.strip_prefix("/nix/store").unwrap())
        )
        .unwrap()
        .is_file()
    );

    tmp.write("homes/test.toml", "name = 'test'\npackages = ['which']\n");
    let home = vug(&[
        "home",
        "build-home",
        "--root",
        tmp.path().to_str().unwrap(),
        "--name",
        "test",
        "--state-dir",
        tmp.path().join("state").to_str().unwrap(),
        "--data-dir",
        data.path().to_str().unwrap(),
    ]);
    assert!(
        home.status.success(),
        "{}",
        String::from_utf8_lossy(&home.stderr)
    );
    let home: serde_json::Value = serde_json::from_slice(&home.stdout).unwrap();
    let package = out.file_name().unwrap().to_str().unwrap();
    let which = vug(&[
        "home",
        "run-program",
        "--home-output",
        home["Path"].as_str().unwrap(),
        "--program",
        &format!("/vug/store/{package}/bin/which"),
        "--",
        "--version",
    ]);
    assert!(
        which.status.success(),
        "{}",
        String::from_utf8_lossy(&which.stderr)
    );
    assert!(String::from_utf8_lossy(&which.stdout).contains("GNU which v2.25"));

    make_dirs_writable(&data.path().join("nix/store"));
}

fn make_dirs_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if std::fs::symlink_metadata(path).unwrap().is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        for entry in std::fs::read_dir(path).unwrap() {
            make_dirs_writable(&entry.unwrap().path());
        }
    }
}

const FHS_FD_RECORD: &str = r#"version = "10.3.0"

[source]
url = "https://github.com/sharkdp/fd/releases/download/v10.3.0/fd-v10.3.0-ARCH-unknown-linux-gnu.tar.gz"
sha256 = "ARCH_SHA256"

[build]
kind = "prebuilt"
install = [
  { from = "fd", to = "bin/fd" },
]

[dependencies]
build = []
run = []

[check]
enabled = false
skip = []

[fhs]
program = "bin/fd"
runtime-x86_64 = [
  { output = "gcc:out", from = "lib/libgcc_s.so.1", to = "usr/lib64/libgcc_s.so.1" },
  { output = "glibc:out", from = "lib", to = "usr/lib64" },
]
graphics = false
"#;

const I686_FD_RUNTIME: &str = r#"runtime-i686 = [
  { output = "gcc-i686:out", from = "lib/libgcc_s.so.1", to = "usr/lib32/libgcc_s.so.1" },
  { output = "glibc-i686:out", from = "lib", to = "usr/lib32" },
]
"#;

fn fd_record(arch: &str) -> String {
    let (sha, runtime) = match arch {
        "x86_64" => (
            "c3c2bc79f838e780173fc8f18b337ec273e7ba17c7ff8f551be29fc3c19b7916",
            "",
        ),
        "i686" => (
            "8bb29df8b6dae820bd4aa9e0c508f458faf988f3353e9d2ceed3fc9d0628c9f7",
            I686_FD_RUNTIME,
        ),
        _ => unreachable!(),
    };
    FHS_FD_RECORD
        .replace("ARCH_SHA256", sha)
        .replace("ARCH", arch)
        .replace("graphics = false", &format!("{runtime}graphics = false"))
}

fn fd_package_dir(tmp: &TmpDir) -> PathBuf {
    let dir = package_dir(tmp, "fd");
    std::fs::write(
        dir.join("package.toml"),
        package_toml("fd").replace(
            "https://example.org/dist/",
            "https://github.com/sharkdp/fd/releases/download/",
        ),
    )
    .unwrap();
    dir
}

#[test]
fn checks_fhs_records() {
    let tmp = TmpDir::new("fhs-record");
    let path = fd_package_dir(&tmp).join("versions/10.3.0.toml");
    let record = fd_record("i686");
    for (old, new, field) in [
        ("bin/fd\"\nruntime", "../fd\"\nruntime", "fhs.program"),
        ("gcc:out", "gcc", "output"),
        ("from = \"lib\"", "from = \"/lib\"", "from"),
        ("usr/lib32", "lib32", "to"),
        ("usr/lib64", "usr/lib32", "to"),
        ("graphics = false", "graphics = 1", "fhs.graphics"),
    ] {
        std::fs::write(&path, record.replace(old, new)).unwrap();
        assert_invalid(&check_record(&path), field);
    }
    std::fs::write(&path, record.replace("graphics = false\n", "")).unwrap();
    let formatted = format_record(&path);
    assert!(formatted.status.success(), "{formatted:?}");
    let output: serde_json::Value = serde_json::from_slice(&formatted.stdout).unwrap();
    assert_eq!(output["Record"].as_str().unwrap(), record);
    std::fs::write(&path, &record).unwrap();
    assert!(check_record(&path).status.success());
    assert_invalid(&check_tree(tmp.path()), "fhs.runtime-x86_64");
    let glibc_i686 = package_dir(&tmp, "glibc-i686").join("versions/2.25.toml");
    std::fs::write(glibc_i686, WHICH_NIX_RECORD).unwrap();
    assert_invalid(
        &check_tree(tmp.path()),
        "expected a nix-output for i686-linux",
    );
}

#[test]
#[ignore = "requires x86_64 Linux with IA32, bubblewrap and access to the upstream archives and cache.nixos.org"]
fn runs_upstream_glibc_archives_in_fhs_home() {
    let tmp = TmpDir::new("fhs-fd");
    let data = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    tmp.write(
        "seed.toml",
        r#"[busybox]
url = "https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox"
sha256 = "6e123e7f3202a8c1e9b1f94d8941580a25135382b99e8d3e34fb858bba311348"

[zig]
url = "https://ziglang.org/download/0.17.1/zig-x86_64-linux-0.17.1.tar.xz"
sha256 = "3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a"
"#,
    );
    for (name, store_path, nar_hash) in [
        (
            "glibc",
            "r3hvhzl5fxhrgn0rfy5g7rda17702pj4-glibc-2.44-57",
            "0h9kdkvc1iv4lj1mvmdvybf1pfr4bf5vsa1nlszhf6h4awymrd12",
        ),
        (
            "glibc-i686",
            "hd6qrzgv3g0079rb7xgkc58q17hxjygd-glibc-2.44-57",
            "1hrfvxmjwj9znh71qx3aypiy9r1yf43hrcwfx5vnsjp5hh8lx0fa",
        ),
        (
            "gcc",
            "qk6cz4ba571znqrsyhma192y8h50yi05-gcc-16.2.0-libgcc",
            "1qp62adnjfrnimm7m8qxv40qip8v5zhmaf9dvmcp0smj8zwn07f4",
        ),
        (
            "gcc-i686",
            "9n2m4riyz8a7i2q692gac6ccjdgpwfg3-gcc-16.2.0-libgcc",
            "1gxarqj1v967v2q7xs6hszflbq7k66z496653qnzh0v591qlcnk1",
        ),
    ] {
        let arch = if name.ends_with("-i686") {
            "i686"
        } else {
            "x86_64"
        };
        let attribute = if name.starts_with("glibc") {
            "glibc"
        } else {
            "stdenv.cc.cc.libgcc"
        };
        let file = package_dir(&tmp, name).join("versions/1.toml");
        std::fs::write(
            file,
            format!(
                r#"version = "1"

[build]
kind = "nix-output"
system = "{arch}-linux"
nixpkgs-rev = "93a9ed0a866099a230ed5146a5bb7d16fd5c9b14"
attribute = "{attribute}"

[build.nix-outputs.out]
store-path = "/nix/store/{store_path}"
nar-hash = "sha256:{nar_hash}"
expose = []

[dependencies]
build = []
run = []

[check]
enabled = false
skip = []
"#
            ),
        )
        .unwrap();
    }
    let dir = fd_package_dir(&tmp);
    tmp.write("homes/test.toml", "name = 'test'\npackages = ['fd']\n");
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vug"))
            .env("XDG_CACHE_HOME", cache.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let root = tmp.path().to_str().unwrap();
    let data_dir = data.path().to_str().unwrap();
    let cache_dir = cache.path().join("vug");
    for arch in ["x86_64", "i686"] {
        let record = fd_record(arch);
        let record = if arch == "i686" {
            record.replace("graphics = false", "graphics = true")
        } else {
            record
        };
        std::fs::write(dir.join("versions/10.3.0.toml"), record).unwrap();
        let tree = check_tree(tmp.path());
        assert!(tree.status.success(), "{tree:?}");
        assert_eq!(
            run(&[
                "package",
                "run-package",
                "--root",
                root,
                "--name",
                "fd",
                "--data-dir",
                data_dir,
                "--cache-dir",
                cache_dir.to_str().unwrap(),
                "--jobs",
                "2",
                "--program",
                "bin/fd",
                "--arguments",
                "--version",
            ])
            .trim(),
            "fd 10.3.0"
        );
        let home = run(&[
            "home",
            "build-home",
            "--root",
            root,
            "--name",
            "test",
            "--data-dir",
            data_dir,
            "--state-dir",
            tmp.path().join("state").to_str().unwrap(),
        ]);
        let home: serde_json::Value = serde_json::from_str(&home).unwrap();
        let output = home["Path"].as_str().unwrap();
        let bytes = std::fs::read(Path::new(output).join("manifest/home.json")).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let fd = manifest["Packages"]["fd"].as_str().unwrap();
        let expected = if arch == "i686" { 5 } else { 3 };
        assert_eq!(manifest["Closure"].as_array().unwrap().len(), expected);
        assert_eq!(
            run(&[
                "home",
                "run-program",
                "--home-output",
                output,
                "--program",
                &format!("/vug/store/{fd}/bin/fd"),
                "--",
                "--version",
            ])
            .trim(),
            "fd 10.3.0"
        );
    }

    make_dirs_writable(&data.path().join("nix/store"));
}
