//! Same-length byte replacement across a directory tree.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Replaces every occurrence of `from` with `to` in the contents of every
/// regular file and in the target of every symbolic link under `root`
/// (`root` itself may be a file, a link, or a directory).
///
/// - `from` and `to` must have the same non-zero length; otherwise the
///   function returns `io::ErrorKind::InvalidInput` and changes nothing.
/// - Returns the total number of occurrences replaced.
/// - Occurrences are found left to right and do not overlap.
/// - A file without an occurrence is not written (its mtime is unchanged).
/// - A rewritten file keeps its permission bits, including for files that
///   are read-only (make it writable for the write, then restore the mode).
/// - A symbolic link with an occurrence in its target is removed and created
///   again with the new target. Links are never followed.
/// - Directory entries are visited in ascending byte order of file name.
pub fn rewrite_tree(root: &Path, from: &[u8], to: &[u8]) -> io::Result<u64> {
    if from.is_empty() || from.len() != to.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "from and to must have the same non-zero length",
        ));
    }
    rewrite_path(root, from, to)
}

fn rewrite_path(path: &Path, from: &[u8], to: &[u8]) -> io::Result<u64> {
    let file_type = fs::symlink_metadata(path)?.file_type();
    if file_type.is_symlink() {
        rewrite_link(path, from, to)
    } else if file_type.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        let mut total = 0;
        for entry in entries {
            total += rewrite_path(&entry.path(), from, to)?;
        }
        Ok(total)
    } else if file_type.is_file() {
        rewrite_file(path, from, to)
    } else {
        Ok(0)
    }
}

fn rewrite_file(path: &Path, from: &[u8], to: &[u8]) -> io::Result<u64> {
    let mut data = fs::read(path)?;
    let count = replace_in_place(&mut data, from, to);
    if count == 0 {
        return Ok(0);
    }
    let mode = fs::metadata(path)?.permissions().mode();
    let made_writable = mode & 0o200 == 0;
    if made_writable {
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o200))?;
    }
    let result = fs::write(path, &data);
    if made_writable {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    result?;
    Ok(count)
}

fn rewrite_link(path: &Path, from: &[u8], to: &[u8]) -> io::Result<u64> {
    let mut target = fs::read_link(path)?.as_os_str().as_bytes().to_vec();
    let count = replace_in_place(&mut target, from, to);
    if count == 0 {
        return Ok(0);
    }
    fs::remove_file(path)?;
    std::os::unix::fs::symlink(OsStr::from_bytes(&target), path)?;
    Ok(count)
}

fn replace_in_place(buf: &mut [u8], from: &[u8], to: &[u8]) -> u64 {
    let mut count = 0;
    let mut i = 0;
    while i + from.len() <= buf.len() {
        if buf[i..].starts_with(from) {
            buf[i..i + from.len()].copy_from_slice(to);
            i += from.len();
            count += 1;
        } else {
            i += 1;
        }
    }
    count
}
