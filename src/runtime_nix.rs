use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use vug::nix_cache::StorePath;

pub(crate) fn command(
    nix_store: &Path,
    nix_paths: &BTreeSet<StorePath>,
    fhs: Option<&crate::fhs::Runtime>,
) -> Result<Command, String> {
    let bwrap = if option_env!("VUG_BUNDLED_BWRAP").is_none() && Path::new("/bin/bwrap").is_file() {
        "/bin/bwrap".into()
    } else {
        crate::build::bwrap()?
    };
    let mut command = Command::new(bwrap);
    command.args(["--unshare-user", "--die-with-parent"]);
    for entry in fs::read_dir("/").map_err(|error| format!("/: {error}"))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if ((!nix_paths.is_empty() && nix_store != Path::new("/nix/store"))
            || (nix_paths.is_empty() && fhs.is_some()))
            && path == Path::new("/nix")
            || fhs.is_some_and(|fhs| fhs.replaces(&path))
        {
            continue;
        }
        let flag = if path == Path::new("/dev") {
            "--dev-bind"
        } else {
            "--bind"
        };
        command.arg(flag).arg(&path).arg(&path);
    }

    if let Some(fhs) = fhs {
        fhs.configure(&mut command)?;
    }
    Ok(command)
}
