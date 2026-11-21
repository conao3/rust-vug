//! Writing a directory tree as a cpio archive in the "newc" format, which is
//! what the Linux kernel unpacks as an initramfs.
//!
//! The archive is a function of the tree alone: entries are in ascending byte
//! order of their names within each directory, every entry is owned by user
//! and group 0, every time stamp is 0, and inode numbers count up from 1.

use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

fn pad(out: &mut impl Write, written: usize) -> io::Result<()> {
    out.write_all(&[0u8; 3][..(4 - written % 4) % 4])
}

fn entry(
    out: &mut impl Write,
    ino: u32,
    mode: u32,
    nlink: u32,
    name: &[u8],
    data: &[u8],
) -> io::Result<()> {
    let fields = [
        ino,
        mode,
        0,
        0,
        nlink,
        0,
        data.len() as u32,
        0,
        0,
        0,
        0,
        name.len() as u32 + 1,
        0,
    ];
    out.write_all(b"070701")?;
    for field in fields {
        write!(out, "{field:08x}")?;
    }
    out.write_all(name)?;
    out.write_all(&[0])?;
    pad(out, 110 + name.len() + 1)?;
    out.write_all(data)?;
    pad(out, data.len())
}

fn walk(out: &mut impl Write, dir: &Path, prefix: &[u8], ino: &mut u32) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for item in entries {
        let path = item.path();
        let mut name = prefix.to_vec();
        if !name.is_empty() {
            name.push(b'/');
        }
        name.extend_from_slice(item.file_name().as_bytes());
        let metadata = fs::symlink_metadata(&path)?;
        let permissions = metadata.permissions().mode() & 0o7777;
        *ino += 1;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(&path)?;
            entry(
                out,
                *ino,
                S_IFLNK | 0o777,
                1,
                &name,
                target.as_os_str().as_bytes(),
            )?;
        } else if file_type.is_dir() {
            entry(out, *ino, S_IFDIR | permissions, 2, &name, &[])?;
            walk(out, &path, &name, ino)?;
        } else if file_type.is_file() {
            entry(
                out,
                *ino,
                S_IFREG | permissions,
                1,
                &name,
                &fs::read(&path)?,
            )?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: unsupported file type", path.display()),
            ));
        }
    }
    Ok(())
}

/// Writes the contents of `root` (not `root` itself) as a newc archive.
pub fn write_newc(root: &Path, out: &mut impl Write) -> io::Result<()> {
    let mut ino = 0;
    walk(out, root, b"", &mut ino)?;
    entry(out, 0, 0, 1, b"TRAILER!!!", &[])?;
    out.flush()
}
