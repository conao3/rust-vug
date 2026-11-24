use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use rustix::fs::{Mode, OFlags, fchmod, mkdirat, openat, symlinkat};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 1_000_000;

pub(super) fn unpack(input: impl Read, parent: &OwnedFd, name: &OsStr) -> io::Result<()> {
    let mut nar = Nar {
        input,
        remaining_nodes: MAX_NODES,
    };
    nar.expect(b"nix-archive-1")?;
    nar.node(parent, name, 0)?;
    if nar.input.read(&mut [0])? != 0 {
        return Err(invalid("trailing bytes after NAR"));
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct Nar<R> {
    input: R,
    remaining_nodes: usize,
}

impl<R: Read> Nar<R> {
    fn length(&mut self) -> io::Result<u64> {
        let mut bytes = [0; 8];
        self.input.read_exact(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn padding(&mut self, length: u64) -> io::Result<()> {
        let mut bytes = [0; 7];
        let padding = ((8 - length % 8) % 8) as usize;
        self.input.read_exact(&mut bytes[..padding])?;
        if bytes != [0; 7] {
            return Err(invalid("nonzero NAR padding"));
        }
        Ok(())
    }

    fn string(&mut self, max_length: usize) -> io::Result<Vec<u8>> {
        let length = self.length()?;
        if length > max_length as u64 {
            return Err(invalid("NAR string exceeds length limit"));
        }
        let mut bytes = vec![0; length as usize];
        self.input.read_exact(&mut bytes)?;
        self.padding(length)?;
        Ok(bytes)
    }

    fn token(&mut self) -> io::Result<Vec<u8>> {
        self.string(16)
    }

    fn expect(&mut self, expected: &[u8]) -> io::Result<()> {
        if self.token()? != expected {
            return Err(invalid(&format!(
                "expected NAR token {:?}",
                String::from_utf8_lossy(expected)
            )));
        }
        Ok(())
    }

    fn node(&mut self, parent: &OwnedFd, name: &OsStr, depth: usize) -> io::Result<()> {
        if depth > MAX_DEPTH {
            return Err(invalid("NAR directory nesting exceeds limit"));
        }
        self.remaining_nodes = self
            .remaining_nodes
            .checked_sub(1)
            .ok_or_else(|| invalid("NAR node count exceeds limit"))?;
        self.expect(b"(")?;
        self.expect(b"type")?;
        match self.token()?.as_slice() {
            b"regular" => self.regular(parent, name),
            b"directory" => self.directory(parent, name, depth),
            b"symlink" => {
                self.expect(b"target")?;
                let target = self.string(4096)?;
                self.expect(b")")?;
                Ok(symlinkat(OsStr::from_bytes(&target), parent, name)?)
            }
            _ => Err(invalid("unsupported NAR node type")),
        }
    }

    fn regular(&mut self, parent: &OwnedFd, name: &OsStr) -> io::Result<()> {
        let mut token = self.token()?;
        let executable = token == b"executable";
        if executable {
            self.expect(b"")?;
            token = self.token()?;
        }
        if token != b"contents" {
            return Err(invalid("expected NAR contents or executable marker"));
        }

        let length = self.length()?;
        let mut file = File::from(openat(
            parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        if io::copy(&mut self.input.by_ref().take(length), &mut file)? != length {
            return Err(invalid("truncated NAR file contents"));
        }
        self.padding(length)?;
        self.expect(b")")?;
        file.flush()?;
        Ok(fchmod(
            &file,
            Mode::from_raw_mode(if executable { 0o755 } else { 0o644 }),
        )?)
    }

    fn directory(&mut self, parent: &OwnedFd, name: &OsStr, depth: usize) -> io::Result<()> {
        mkdirat(parent, name, Mode::RWXU)?;
        let dir = openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let mut previous = Vec::new();
        loop {
            match self.token()?.as_slice() {
                b")" => return Ok(fchmod(&dir, Mode::from_raw_mode(0o755))?),
                b"entry" => {}
                _ => return Err(invalid("expected NAR directory entry")),
            }
            self.expect(b"(")?;
            self.expect(b"name")?;
            let name = self.string(255)?;
            if name.is_empty()
                || name == b"."
                || name == b".."
                || name.contains(&b'/')
                || name.contains(&0)
            {
                return Err(invalid("invalid NAR entry name"));
            }
            if name <= previous {
                return Err(invalid("duplicate or unsorted NAR entry"));
            }
            self.expect(b"node")?;
            self.node(&dir, OsStr::from_bytes(&name), depth + 1)?;
            self.expect(b")")?;
            previous = name;
        }
    }
}

pub(super) fn dump(path: &Path, output: &mut impl Write) -> io::Result<()> {
    write_string(output, b"nix-archive-1")?;
    let mut remaining = MAX_NODES;
    dump_node(path, output, 0, &mut remaining)
}

fn write_string(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let len = bytes.len() as u64;
    output.write_all(&len.to_le_bytes())?;
    output.write_all(bytes)?;
    write_padding(output, len)
}

fn write_padding(output: &mut impl Write, len: u64) -> io::Result<()> {
    output.write_all(&[0; 7][..((8 - len % 8) % 8) as usize])
}

fn dump_node(
    path: &Path,
    output: &mut impl Write,
    depth: usize,
    remaining: &mut usize,
) -> io::Result<()> {
    if depth > MAX_DEPTH || *remaining == 0 {
        return Err(invalid("installed NAR exceeds tree limits"));
    }
    *remaining -= 1;
    let metadata = fs::symlink_metadata(path)?;
    let mode = metadata.permissions().mode() & 0o7777;
    write_string(output, b"(")?;
    write_string(output, b"type")?;
    if metadata.is_file() && matches!(mode, 0o444 | 0o555) {
        write_string(output, b"regular")?;
        if mode == 0o555 {
            write_string(output, b"executable")?;
            write_string(output, b"")?;
        }
        write_string(output, b"contents")?;
        let file = File::open(path)?;
        let len = metadata.len();
        output.write_all(&len.to_le_bytes())?;
        if io::copy(&mut file.take(len.saturating_add(1)), output)? != len {
            return Err(invalid("file size changed while verifying installed NAR"));
        }
        write_padding(output, len)?;
    } else if metadata.is_symlink() {
        write_string(output, b"symlink")?;
        write_string(output, b"target")?;
        write_string(output, fs::read_link(path)?.as_os_str().as_bytes())?;
    } else if metadata.is_dir() && mode == 0o555 {
        write_string(output, b"directory")?;
        let mut entries = fs::read_dir(path)?
            .take(*remaining + 1)
            .collect::<io::Result<Vec<_>>>()?;
        if entries.len() > *remaining {
            return Err(invalid("installed NAR exceeds node limit"));
        }
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            write_string(output, b"entry")?;
            write_string(output, b"(")?;
            write_string(output, b"name")?;
            write_string(output, entry.file_name().as_bytes())?;
            write_string(output, b"node")?;
            dump_node(&entry.path(), output, depth + 1, remaining)?;
            write_string(output, b")")?;
        }
    } else {
        return Err(invalid(
            "unexpected file type or permissions in installed NAR",
        ));
    }
    write_string(output, b")")
}

pub(super) fn freeze(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            freeze(&entry?.path())?;
        }
    }
    let mode = metadata.permissions().mode() & 0o555;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

pub(super) fn make_dirs_writable(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        for entry in fs::read_dir(path)? {
            make_dirs_writable(&entry?.path())?;
        }
    }
    Ok(())
}
