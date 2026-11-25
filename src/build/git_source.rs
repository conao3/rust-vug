use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest as _, Sha256};

pub(super) fn fetch(url: &str, rev: &str, expected_hash: &str, dest: &Path) -> Result<(), String> {
    let repo = tempfile::tempdir_in(dest.parent().unwrap())
        .map_err(|error| format!("create temporary Git repository: {error}"))?;
    let mut git = Command::new("git");
    let env_names = run(
        git.args(["rev-parse", "--local-env-vars"]),
        "read local environment names",
    )?;
    let env_names = std::str::from_utf8(&env_names).map_err(|error| error.to_string())?;
    let git = || {
        let mut git = Command::new("git");
        for name in env_names.lines().filter(|name| {
            !matches!(
                *name,
                "GIT_CONFIG" | "GIT_CONFIG_PARAMETERS" | "GIT_CONFIG_COUNT"
            )
        }) {
            git.env_remove(name);
        }
        git.arg("--git-dir")
            .arg(repo.path())
            .arg("--work-tree")
            .arg(dest);
        git.current_dir(dest);
        git.args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.symlinks=true",
            "-c",
            "fetch.fsckObjects=true",
            "-c",
            "protocol.ext.allow=never",
        ]);
        git
    };
    run(
        git().args(["init", "--object-format=sha1", "--template="]),
        "initialize temporary repository",
    )?;
    let ssh_url = url.strip_prefix("git+").unwrap_or(url);
    run(
        git().args([
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--depth=1",
            "--",
            ssh_url,
            rev,
        ]),
        "fetch pinned revision",
    )?;
    let actual_rev = run(
        git().args(["rev-parse", "--verify", "FETCH_HEAD^{commit}"]),
        "resolve fetched commit",
    )?;
    let actual_rev = String::from_utf8_lossy(&actual_rev);
    if actual_rev.trim_end() != rev {
        return Err(format!(
            "Git commit mismatch: expected {rev}, got {}",
            actual_rev.trim_end()
        ));
    }

    let info = repo.path().join("info");
    super::io("create", &info, fs::create_dir_all(&info))?;
    let attributes = info.join("attributes");
    super::io(
        "disable Git checkout filters",
        &attributes,
        fs::write(
            &attributes,
            "* -text -crlf -ident !filter !working-tree-encoding\n",
        ),
    )?;
    run(git().args(["read-tree", rev]), "read pinned tree")?;
    let entries = run(
        git().args(["ls-files", "--stage", "-z"]),
        "list tree entries",
    )?;
    if entries
        .split(|&b| b == 0)
        .any(|entry| entry.starts_with(b"160000 "))
    {
        return Err("Git source contains a submodule".to_owned());
    }
    run(
        git().args(["checkout-index", "--all"]),
        "materialize pinned tree",
    )?;
    let actual_hash = super::io("hash Git source tree", dest, nar_hash(dest))?;
    if actual_hash != expected_hash {
        return Err(format!(
            "Git source NAR hash mismatch: expected {expected_hash}, got {actual_hash}"
        ));
    }
    Ok(())
}

fn run(command: &mut Command, action: &str) -> Result<Vec<u8>, String> {
    let result = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Git {action}: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "Git {action}: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(result.stdout)
}

fn nar_hash(root: &Path) -> io::Result<String> {
    let mut hash = HashWriter(Sha256::new());
    nar_string(&mut hash, b"nix-archive-1")?;
    nar_node(&mut hash, root, root)?;
    Ok(format!("sha256-{}", STANDARD.encode(hash.0.finalize())))
}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn nar_string(out: &mut impl Write, value: &[u8]) -> io::Result<()> {
    out.write_all(&(value.len() as u64).to_le_bytes())?;
    out.write_all(value)?;
    nar_padding(out, value.len() as u64)
}

fn nar_padding(out: &mut impl Write, len: u64) -> io::Result<()> {
    out.write_all(&[0; 7][..((8 - len % 8) % 8) as usize])
}

fn nar_node(out: &mut impl Write, root: &Path, path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    nar_string(out, b"(")?;
    nar_string(out, b"type")?;
    if metadata.is_dir() {
        nar_string(out, b"directory")?;
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            nar_string(out, b"entry")?;
            nar_string(out, b"(")?;
            nar_string(out, b"name")?;
            nar_string(out, entry.file_name().as_bytes())?;
            nar_string(out, b"node")?;
            nar_node(out, root, &entry.path())?;
            nar_string(out, b")")?;
        }
    } else if metadata.is_file() {
        nar_string(out, b"regular")?;
        if metadata.permissions().mode() & 0o100 != 0 {
            nar_string(out, b"executable")?;
            nar_string(out, b"")?;
        }
        nar_string(out, b"contents")?;
        out.write_all(&metadata.len().to_le_bytes())?;
        if io::copy(&mut File::open(path)?, out)? != metadata.len() {
            return Err(io::Error::other(
                "file size changed while hashing Git source",
            ));
        }
        nar_padding(out, metadata.len())?;
    } else if metadata.is_symlink() {
        nar_string(out, b"symlink")?;
        nar_string(out, b"target")?;
        let target = fs::read_link(path)?;
        let relative = path.strip_prefix(root).unwrap();
        if !super::source_zip::safe_target(relative, &target) {
            return Err(io::Error::other(format!(
                "unsafe Git symlink: {}",
                relative.display()
            )));
        }
        nar_string(out, target.as_os_str().as_bytes())?;
    } else {
        return Err(io::Error::other("unsupported file type in Git source"));
    }
    nar_string(out, b")")
}
