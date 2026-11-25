use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use vug::{fetch, treehash, wrapper};

const URL: &str = "git+ssh://git@example.invalid/packages/source.git";
const NAR_HASH: &str = "sha256-nnEdj2PmhI2/syMABzLvGMTbhMwSRu/o9HCXzPbwkaU=";
const WRONG_NAR_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const WRONG_REV: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn success(output: Output) -> Output {
    assert!(output.status.success(), "{output:?}");
    output
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = success(
        Command::new("git")
            .current_dir(repo)
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap(),
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn record(root: &Path, name: &str, source: &str, dependency: &str, split: bool) -> PathBuf {
    let package = root.join("packages").join(&name[..2]).join(name);
    let versions = package.join("versions");
    fs::create_dir_all(&versions).unwrap();
    fs::write(
        package.join("package.toml"),
        format!("name = \"{name}\"\nlicense = \"MIT\"\n\n[upstream]\nsources = [\n  \"git+ssh://git@example.invalid/packages/\",\n  \"https://example.invalid/\",\n]\n"),
    )
    .unwrap();
    let path = versions.join("1.toml");
    let outputs = if split {
        "\n[outputs.dev]\npaths = [\n  \"include/**\",\n]\nrun = []\n\n[outputs.out]\nrun = []\n"
    } else {
        ""
    };
    fs::write(
        &path,
        format!(
            r#"version = "1"

[source]
{source}

[build]
kind = "commands"
commands = [
  ["true"],
]

[dependencies]
{dependency}
{outputs}
[check]
enabled = false
skip = []
"#
        ),
    )
    .unwrap();
    path
}

fn install_sandbox_fixture(tools: &Path) {
    fs::create_dir(tools).unwrap();
    let path = std::env::var_os("PATH").unwrap();
    let sh = std::env::split_paths(&path)
        .map(|directory| directory.join("sh"))
        .find(|path| path.is_file())
        .unwrap();
    let bwrap = tools.join("bwrap");
    fs::write(
        &bwrap,
        format!(
            r#"#!{}
[ "$1" = --unshare-all ] || exit 1
while [ "$#" -gt 2 ]; do
    case "$1:$3" in
        --bind:/build) source=$2/src ;;
        --bind:/vug/store/*) output=$2 ;;
    esac
    shift
done
[ ! -e "$source/.git" ] || exit 1
if [ -e "$source/payload" ]; then
    [ -x "$source/nested/executable" ] && [ -L "$source/nested/link" ] || exit 1
    IFS= read -r payload < "$source/nested/link" || exit 1
    printf '%s\n' "$payload" > "$output/payload"
else
    printf 'archive' > "$output/payload"
fi
mkdir "$output/include"
printf 'header' > "$output/include/header"
"#,
            sh.display()
        ),
    )
    .unwrap();
    fs::set_permissions(bwrap, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn verifies_git_source_and_excludes_its_transitive_outputs_from_export() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let repo = root.join("upstream");
    fs::create_dir_all(repo.join("nested")).unwrap();
    git(&repo, &["init", "--object-format=sha1", "--template="]);
    fs::write(repo.join("payload"), b"payload\r\n").unwrap();
    let executable = repo.join("nested/executable");
    fs::write(&executable, b"nested file\n").unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    symlink("../payload", repo.join("nested/link")).unwrap();
    git(&repo, &["add", "payload", "nested"]);
    fs::write(
        repo.join(".gitattributes"),
        "payload text eol=lf filter=fixture\n",
    )
    .unwrap();
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["commit", "-m", "Pinned tree"]);
    let rev = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["tag", "-a", "-m", "Annotated tag", "fixture"]);
    let tag = git(&repo, &["rev-parse", "fixture"]);
    fs::write(repo.join("payload"), b"later commit\n").unwrap();
    git(&repo, &["add", "payload"]);
    git(&repo, &["commit", "-m", "Advance the branch"]);

    let data = root.join("data");
    let cache = root.join("cache");
    let tools = root.join("tools");
    install_sandbox_fixture(&tools);
    let seed = "";
    fs::write(root.join("seed.toml"), seed).unwrap();
    let mut seed_hash = blake3::Hasher::new();
    for input in [
        seed,
        wrapper::COMPILER_WRAPPER,
        wrapper::TOOL_WRAPPER,
        wrapper::CPP_WRAPPER,
        "seed-layout=2",
    ] {
        seed_hash.update(input.as_bytes());
    }
    let seed_id = treehash::encode_base32(seed_hash.finalize().as_bytes().first_chunk().unwrap());
    fs::create_dir_all(data.join("seed").join(&seed_id).join("bin")).unwrap();

    let archive = b"unchanged public source";
    let digest = fetch::sha256_reader(archive.as_slice()).unwrap();
    fs::create_dir_all(cache.join("sources")).unwrap();
    fs::write(cache.join("sources").join(&digest), archive).unwrap();
    let archive_source = format!("url = \"https://example.invalid/source\"\nsha256 = \"{digest}\"");
    let no_deps = "build = []\nrun = []";
    let public_record = record(root, "public", &archive_source, no_deps, false);
    let git = |name, revision: &str, hash, split| {
        record(
            root,
            name,
            &format!("url = \"{URL}\"\nrev = \"{revision}\"\nnar-hash = \"{hash}\""),
            no_deps,
            split,
        )
    };
    let valid_record = git("private", &rev, NAR_HASH, true);
    let bad_rev_record = git("invalid-revision", "HEAD", NAR_HASH, false);
    let missing_rev = git("missing-commit", WRONG_REV, NAR_HASH, false);
    git("tag-object", &tag, NAR_HASH, false);
    git("wrong-hash", &rev, WRONG_NAR_HASH, false);
    record(
        root,
        "exposure",
        &archive_source,
        "build = []\nrun = []\nexpose = [\n  \"private:dev\",\n]",
        false,
    );
    record(
        root,
        "dependent",
        &archive_source,
        "build = [\n  \"exposure\",\n]\nrun = []",
        true,
    );

    let cli = || {
        let path = std::env::var_os("PATH").unwrap();
        let path = std::env::join_paths(
            std::iter::once(tools.clone()).chain(std::env::split_paths(&path)),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vug"));
        let local_url = url::Url::from_file_path(&repo).unwrap();
        cmd.env("PATH", path)
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", format!("url.{local_url}.insteadOf"))
            .env("GIT_CONFIG_VALUE_0", URL.strip_prefix("git+").unwrap())
            .env("GIT_CONFIG_KEY_1", "filter.fixture.required")
            .env("GIT_CONFIG_VALUE_1", "true");
        cmd
    };
    let build = |name: &str| {
        cli()
            .args(["package", "build-package", "--name", name, "--root"])
            .arg(root)
            .arg("--data-dir")
            .arg(&data)
            .arg("--cache-dir")
            .arg(&cache)
            .output()
            .unwrap()
    };
    let check = |path: &Path| {
        cli()
            .args(["record", "check-record", "--file"])
            .arg(path)
            .output()
            .unwrap()
    };
    success(check(&valid_record));
    let invalid = check(&bad_rev_record);
    assert_eq!(invalid.status.code(), Some(254), "{invalid:?}");
    assert!(
        String::from_utf8_lossy(&invalid.stdout).contains("source.rev"),
        "{invalid:?}"
    );
    let tree = cli()
        .args(["record", "check-tree", "--root"])
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(tree.status.code(), Some(254), "{tree:?}");
    assert!(
        String::from_utf8_lossy(&tree.stdout).contains("source.rev"),
        "{tree:?}"
    );
    fs::remove_file(bad_rev_record).unwrap();
    fs::remove_dir_all(root.join("packages/in/invalid-revision")).unwrap();
    success(
        cli()
            .args(["record", "check-tree", "--root"])
            .arg(root)
            .output()
            .unwrap(),
    );

    for (name, message) in [
        ("missing-commit", "fetch pinned revision"),
        ("tag-object", "Git commit mismatch"),
        ("wrong-hash", "Git source NAR hash mismatch"),
    ] {
        let failure = build(name);
        assert_eq!(failure.status.code(), Some(254), "{failure:?}");
        assert!(
            String::from_utf8_lossy(&failure.stderr).contains(message),
            "{failure:?}"
        );
        assert!(fs::read_dir(data.join("tmp")).unwrap().next().is_none());
    }
    fs::remove_file(missing_rev).unwrap();

    for name in ["private", "dependent", "exposure", "public"] {
        success(build(name));
    }
    let output_manifest = |name| -> Value {
        serde_json::from_slice(&fs::read(data.join(format!("index-outputs/{name}/1"))).unwrap())
            .unwrap()
    };
    for name in ["private", "dependent"] {
        let manifest = output_manifest(name);
        assert_eq!(manifest["private"], true);
        for output in manifest["outputs"].as_object().unwrap().values() {
            let tree = data.join("store").join(output["id"].as_str().unwrap());
            assert!(tree.join(".vug/private-source").is_file());
        }
    }
    let direct = output_manifest("private");
    let private_tree = data
        .join("store")
        .join(direct["outputs"]["out"]["id"].as_str().unwrap());
    assert_eq!(
        fs::read(private_tree.join("payload")).unwrap(),
        b"payload\r\n"
    );
    let exposed = fs::read_to_string(data.join("index/exposure/1")).unwrap();
    assert!(exposed.ends_with(" private\n"));

    let legacy_index = fs::read_to_string(data.join("index/public/1")).unwrap();
    let (key, public_id) = legacy_index.trim_end().split_once(' ').unwrap();
    let canonical = fs::read_to_string(&public_record).unwrap();
    let expected_key = blake3::hash(format!("{canonical}{seed_id}10").as_bytes());
    assert_eq!(
        key,
        treehash::encode_base32(expected_key.as_bytes().first_chunk().unwrap())
    );
    assert_eq!(
        fs::read_to_string(data.join("store").join(public_id).join(".vug/record.toml")).unwrap(),
        canonical
    );

    let staging = root.join("staging");
    let result = success(
        cli()
            .args(["store", "export-cache", "--data-dir"])
            .arg(&data)
            .arg("--directory")
            .arg(&staging)
            .output()
            .unwrap(),
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["Keys"], 1);
    assert_eq!(report["Written"], 1);
    assert!(staging.join("keys").join(key).is_file());
    assert!(
        staging
            .join("paths")
            .join(format!("{public_id}.cpio.zst"))
            .is_file()
    );
    assert!(
        fs::read_dir(staging.join("output-keys"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(fs::read_dir(data.join("tmp")).unwrap().next().is_none());

    let cached = success(build("private"));
    assert_eq!(
        serde_json::from_slice::<Value>(&cached.stdout).unwrap()["Built"],
        false
    );
}
