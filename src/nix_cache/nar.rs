use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::{ffi::OsStr, fs::File};

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
