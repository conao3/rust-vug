use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const VERSION_PATH: &str = "packages/he/hello/versions/1.toml";
const VERSION: &str = r#"
version = "1"

[source]
url = "https://example.org/dist/hello-1.tar.gz"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[build]
kind = "autotools"

[dependencies]
build = []
run = []

[check]
enabled = true
skip = []
"#;

fn request(path: &str, before: Value, after: Value, targets: &[(u8, &str)]) -> Value {
    let identity = json!({
        "base_sha": "1".repeat(40),
        "head_sha": "2".repeat(40),
        "tree_oid": "3".repeat(40),
        "policy_digest": "4".repeat(64),
    });
    let required: Vec<_> = targets
        .iter()
        .map(|(check, target)| json!({"check": check, "target": target}))
        .collect();
    let results: Vec<_> = targets
        .iter()
        .map(|(check, target)| {
            json!({"identity": identity, "check": check, "target": target, "status": "passed"})
        })
        .collect();
    json!({
        "schema_version": 1,
        "identity": identity,
        "core_packages": [],
        "changes": [{"path": path, "before": before, "after": after}],
        "required_checks": required,
        "results": results,
    })
}

fn blob(text: &str) -> Value {
    json!({"mode": "100644", "text": text})
}

fn version_request(before: Value, after: Value) -> Value {
    let targets = [1, 3, 4, 5, 6, 7, 8].map(|check| (check, "hello"));
    request(VERSION_PATH, before, after, &targets)
}

fn decide(request: &Value) -> Value {
    let mut cli = Command::new(env!("CARGO_BIN_EXE_vug"))
        .args(["record", "decide-merge", "--input", "file:///dev/stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    write!(cli.stdin.take().unwrap(), "{request}").unwrap();
    let output = cli.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_verdict(request: &Value, class: &str, scope: &str, decision: &str) -> Value {
    let report = decide(request);
    assert_eq!(report["ChangeClass"], class);
    assert_eq!(report["Scope"], scope);
    assert_eq!(report["Decision"], decision);
    report
}

#[test]
fn classifies_package_additions_and_compares_normalized_source() {
    assert_verdict(
        &version_request(Value::Null, blob(VERSION)),
        "record_change",
        "passed",
        "auto_merge",
    );
    let explicitly_default = VERSION.replace("[build]", "strip-components = 1\n\n[build]");
    assert_verdict(
        &version_request(blob(VERSION), blob(&explicitly_default)),
        "record_change",
        "passed",
        "auto_merge",
    );
    let changed = VERSION.replace("example.org", "mirror.example.org");
    for (before, after, reason) in [
        (blob(VERSION), blob(&changed), "existing_source_changed"),
        (blob(VERSION), Value::Null, "version_deleted"),
        (
            blob(VERSION),
            blob(&VERSION.replace("enabled = true", "enabled = false")),
            "tests_disabled",
        ),
    ] {
        let report = assert_verdict(
            &version_request(before, after),
            "approval_change",
            "requires_approval",
            "requires_approval",
        );
        assert_eq!(
            report["Reasons"],
            json!([{"Code": reason, "Path": VERSION_PATH}])
        );
    }
}

#[test]
fn classifies_repository_changes_and_rejects_unsupported_modes() {
    let mut request = request(
        "packages-extra/file",
        Value::Null,
        json!({"mode": "100644"}),
        &[(1, ".")],
    );
    assert_verdict(
        &request,
        "repository_change",
        "requires_approval",
        "requires_approval",
    );

    request["changes"][0]["after"]["mode"] = json!("120000");
    let report = assert_verdict(&request, "repository_change", "failed", "reject");
    assert!(
        report["Reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason["Code"] == "unsupported_git_mode")
    );
}

#[test]
fn reports_missing_targets_and_incomplete_results() {
    let mut request = version_request(Value::Null, blob(VERSION));
    request["required_checks"].as_array_mut().unwrap().pop();
    request["results"].as_array_mut().unwrap().pop();
    request["results"][0]["status"] = json!("incomplete");
    request["results"].as_array_mut().unwrap().remove(1);

    let report = assert_verdict(&request, "record_change", "passed", "incomplete");
    assert_eq!(
        report["IncompleteChecks"],
        json!([
            {"Check": 1, "Target": "hello"},
            {"Check": 3, "Target": "hello"},
            {"Check": 8, "Target": "hello"},
        ])
    );
}

#[test]
fn rejects_failed_duplicate_unexpected_and_stale_results() {
    for (field, value, reason) in [
        ("status", json!("failed"), "check_failed"),
        ("target", json!("wrong"), "unexpected_result"),
        ("check", json!(3), "duplicate_result"),
        (
            "identity",
            json!({
                "base_sha": "5".repeat(40),
                "head_sha": "2".repeat(40),
                "tree_oid": "3".repeat(40),
                "policy_digest": "4".repeat(64),
            }),
            "result_identity_mismatch",
        ),
    ] {
        let mut request = version_request(Value::Null, blob(VERSION));
        request["results"][0][field] = value;
        let report = assert_verdict(&request, "record_change", "passed", "reject");
        assert!(
            report["Reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["Code"] == reason)
        );
        request["results"].as_array_mut().unwrap().reverse();
        assert_eq!(report, decide(&request));
    }
}

#[test]
fn requires_approval_for_unapproved_patch_provenance() {
    let mut request = version_request(Value::Null, blob(VERSION));
    request["results"][3]["status"] = json!("requires_approval");
    assert_verdict(&request, "approval_change", "passed", "requires_approval");
}
