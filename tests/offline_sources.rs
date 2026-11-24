use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use vug::{fetch, treehash};
use zip::{ZipWriter, write::SimpleFileOptions};

fn cache_file(cache: &Path, bytes: &[u8]) -> String {
    let digest = fetch::sha256_reader(bytes).unwrap();
    fs::write(cache.join(&digest), bytes).unwrap();
    digest
}

#[test]
fn builds_with_verified_bytes_and_rejects_missing_or_corrupt_sources_before_sandbox() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let cache = root.join("verified");
    let data = root.join("data");
    let tools = root.join("tools");
    let versions = root.join("packages/de/demo/versions");
    for path in [&cache, &tools, &versions] {
        fs::create_dir_all(path).unwrap();
    }
    let seed_digest = cache_file(&cache, b"seed fixture");
    let seed = format!(
        "[busybox]\nurl = 'https://example.invalid/busybox'\nsha256 = '{seed_digest}'\n\n[zig]\nurl = 'https://example.invalid/zig'\nsha256 = '{seed_digest}'\n"
    );
    fs::write(root.join("seed.toml"), &seed).unwrap();
    let mut seed_hash = blake3::Hasher::new();
    seed_hash.update(seed.as_bytes());
    for wrapper in [
        vug::wrapper::COMPILER_WRAPPER,
        vug::wrapper::TOOL_WRAPPER,
        vug::wrapper::CPP_WRAPPER,
    ] {
        seed_hash.update(wrapper.as_bytes());
    }
    seed_hash.update(b"seed-layout=2");
    let seed_id =
        treehash::encode_base32(seed_hash.finalize().as_bytes()[..20].try_into().unwrap());
    fs::create_dir_all(data.join("seed").join(seed_id).join("bin")).unwrap();

    fs::write(
        root.join("packages/de/demo/package.toml"),
        r#"name = "demo"
license = "MIT"
[upstream]
sources = ["https://example.invalid/"]
"#,
    )
    .unwrap();
    let archive = root.join("source.zip");
    let mut zip = ZipWriter::new(File::create(&archive).unwrap());
    zip.start_file("source/payload", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"verified payload").unwrap();
    zip.finish().unwrap();
    let bytes = fs::read(&archive).unwrap();
    let digest = fetch::sha256_reader(bytes.as_slice()).unwrap();
    fs::write(
        versions.join("1.toml"),
        format!(
            r#"version = "1"

[source]
url = "https://example.invalid/source.zip"
sha256 = "{digest}"

[build]
kind = "commands"
commands = [["true"]]

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

    let path = std::env::var_os("PATH").unwrap();
    let sh = std::env::split_paths(&path)
        .map(|dir| dir.join("sh"))
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
        --bind:/build) source=$2/src/payload ;;
        --bind:/vug/store/*) output=$2/payload ;;
    esac
    shift
done
IFS= read -r payload < "$source" || [ -n "$payload" ] || exit 1
printf '%s' "$payload" > "$output"
"#,
            sh.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&bwrap, fs::Permissions::from_mode(0o755)).unwrap();
    let path =
        std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(&path))).unwrap();
    let build = || {
        Command::new(env!("CARGO_BIN_EXE_vug"))
            .args(["package", "build-package", "--name", "demo", "--root"])
            .arg(root)
            .arg("--data-dir")
            .arg(&data)
            .arg("--source-cache-dir")
            .arg(&cache)
            .env("PATH", &path)
            .output()
            .unwrap()
    };

    let missing = build();
    assert_eq!(missing.status.code(), Some(254));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("verify offline source"),
        "{missing:?}"
    );
    assert!(!data.join("tmp").exists());

    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    fs::write(cache.join(&digest), corrupt).unwrap();
    let mismatch = build();
    assert_eq!(mismatch.status.code(), Some(254));
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("expected"));
    assert!(!data.join("tmp").exists());

    cache_file(&cache, &bytes);
    let result = build();
    assert!(result.status.success(), "{:?}", result);
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["Built"], true);
    let installed = Path::new(report["HostPath"].as_str().unwrap()).join("payload");
    assert_eq!(fs::read(installed).unwrap(), b"verified payload");
}
