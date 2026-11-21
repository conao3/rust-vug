# Building a package

`vug package build-package` turns one version record into one store path.
The build runs in an empty root that holds only the seed, the dependencies,
and the source.

## Store paths

- A store path is `/vug/store/<id>`, where `<id>` is 32 characters of
  lowercase RFC 4648 base32 (`a-z2-7`, no padding). Every store path is 43
  characters long.
- `<id>` encodes the first 20 bytes of the BLAKE3 hash of the output tree.
- The name and version of a package are not part of the path. They are read
  from `<path>/.vug/record.toml`, a copy of the version record that produced
  the output.

## Tree hash

The output tree is serialized to a byte stream and hashed. All integers are
u64 little-endian.

| Entry | Serialization |
|---|---|
| Regular file | `F`, then `x` when the owner-execute bit is set and `-` otherwise, the content length, the content |
| Symbolic link | `L`, the target length, the target |
| Directory | `D`, the number of entries, then for each entry in ascending byte order of its name: the name length, the name, the entry |

Timestamps, owners, and other mode bits do not affect the hash. Any other
file type is an error.

## Self-references

An output may contain its own path (an `rpath`, a script's interpreter line).
The build therefore installs into a placeholder path of the same length:

1. The output is built with the prefix `/vug/store/` followed by 32 `0`
   characters (`0` is not in the base32 alphabet, so a placeholder never
   equals a real id).
2. The tree hash of the output, placeholder included, gives `<id>`.
3. Every occurrence of the placeholder in file contents and link targets is
   replaced by `/vug/store/<id>`.

Checking a store path reverses step 3 and compares the hash with the id.

## Steps of a build

1. Fetch `source.url` into the cache directory, keyed by `source.sha256`. A
   file whose SHA-256 differs from the record is rejected.
2. Create the build root: the seed tools, each dependency at its store path,
   the source archive, the patches, and an empty output directory at the
   placeholder path.
3. Run the build inside `bwrap --unshare-all --clearenv`: no network, no
   host file system, a root that is read-only outside the build directory,
   `/tmp`, and the output, and only the environment variables `PATH`, `HOME`,
   `SOURCE_DATE_EPOCH`, `ZIG_GLOBAL_CACHE_DIR`, and `ZIG_LOCAL_CACHE_DIR`.
4. Unpack with `tar -xf <archive> --strip-components=<n>` and apply the
   patches with `patch -p1`.
5. Run the steps of the build kind (`docs/record.md`).
6. Copy the version record to `.vug/record.toml` in the output.
7. Hash the output, replace the placeholder, and move the output to its
   store path.

## The seed

`seed.toml` at the root of the package tree names the two upstream binaries
every build starts from:

```toml
[busybox]
url = "https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox"
sha256 = "6e123e7f3202a8c1e9b1f94d8941580a25135382b99e8d3e34fb858bba311348"

[zig]
url = "https://ziglang.org/download/0.17.1/zig-x86_64-linux-0.17.1.tar.xz"
sha256 = "3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a"
```

The seed is assembled once per content of `seed.toml` and bound read-only at
`/seed`:

- `bin/busybox` and one link per busybox applet (`sh`, `tar`, `sed`, ...).
- `zig/`, the unpacked zig distribution.
- `bin/cc` and `bin/c++` run `zig cc` and `zig c++` for
  `x86_64-linux-musl`. They turn the spellings that packages use into what
  zig accepts:
  - `--target=...` and `-mtune=generic` are dropped.
  - `-isystem <dir>` becomes `-I<dir>`: zig searches its own C library and
    kernel headers before every `-isystem` directory, so a package could not
    put its own copy of such a header first.
  - `-march=x86-64` becomes `-march=x86_64` (and `-v2` to `-v4` likewise).
  - `-m16` also enables the 16-bit mode feature, which zig switches off.
  - `-mno-sse` also disables SSE2 and later, which zig leaves on.
  - `-Wl,-R<dir>` becomes `-Wl,-rpath,<dir>` and `-Wl,-h<name>` becomes
    `-Wl,-soname,<name>`; `-stats`, `-lc`, and `-shared`
    inside `-Wl,` are dropped.
  - `-S` together with a dependency file (`-MD`, `-MMD`) runs zig twice,
    because zig writes no assembly output for that combination.
  - a relocatable object whose file name ends in `.so` is passed under a
    name ending in `.o`, because zig takes any `.so` input for a shared
    library and then fails to link.
  - input from standard input (`-`) without `-x` is given `-x c` (`-x c++`
    for `c++`); without it zig does not search the C library headers.
  - a link whose inputs are only libraries gets an empty source file as
    well; zig cannot link without an object of its own.
  - `-fno-sanitize=undefined` comes before the arguments of the package:
    without an optimization level zig compiles with the undefined-behavior
    sanitizer, and a program then aborts where the same code built by
    another compiler runs.
  - an unused command-line argument and a use of `__DATE__` or `__TIME__`
    are never errors (zig makes the second one an error at `-O3`; with
    `SOURCE_DATE_EPOCH` set the macros are reproducible).
- `bin/ar`, `bin/ranlib`, and `bin/ld` run the matching
  zig tools; `bin/gcc` links to `cc`.

## `/bin` and `/usr/bin`

Scripts name their interpreter by a fixed path (`#!/bin/sh`, `#!/bin/bash`,
`#!/usr/bin/env python3`). The build root therefore has one directory of
links, shown at both `/bin` and `/usr/bin`: a link for every program of the
seed, and a link for every program in the `bin` directory of each dependency
in the build root. A dependency's program replaces a seed program of the same
name, so a packaged `sed` or `tar` is the one that runs once it is a
dependency.

## How a build finds its dependencies

The compiler wrappers add flags from two environment variables that the
build sets from the dependencies in the build root, after the arguments of
the package itself, so a directory the package names is searched before the
directories of its dependencies: `VUG_CFLAGS` on every
call, and `VUG_LDFLAGS` on calls that link (no `-c`, `-E`, `-S`, `-M`, or
`-MM`). What a dependency contributes is decided by the files it holds:

| The dependency holds | Effect |
|---|---|
| `bin/` | Added to `PATH`. |
| `lib/ld-musl-*.so.1` (a C library with its dynamic loader) | Programs are linked dynamically with that loader: `-fPIC` when compiling, `-dynamic -Wl,--dynamic-linker=<loader>` when linking. Nothing else of this dependency is added. |
| `include/` | `-isystem <path>/include`: a system header directory, so a warning inside a header of a dependency is not reported (and is no error under `-Werror`). The compiler searches its own C library and kernel headers first. |
| `lib/` | `-L<path>/lib`, and `-Wl,-rpath,<path>/lib` when the dependency is needed at run time (it is in `dependencies.run`, or a run-time dependency of one that is). |
| `lib/pkgconfig/`, `share/pkgconfig/` | Added to `PKG_CONFIG_PATH`. |
| `bin/pkgconf` | `PKG_CONFIG` is set to it. |
| `lib/python*/site-packages/` | Added to `PYTHONPATH`. |

Two flags are always added when linking: `-Wl,-rpath,<output>/lib`, so a
program finds the libraries of its own package, and
`-Wl,--undefined-version`, so a linker version script may name symbols that
the library does not define (the seed linker rejects that by default, and
many packages rely on it).

A package that does not depend on a C library with a loader is linked
statically against the C library bundled with the seed compiler.

## Dependencies and the index

Each name in `dependencies.build` and `dependencies.run` is built first, at
its current version. The build root holds every dependency together with
what that dependency needs at run time, each at its store path, and `PATH`
lists their `bin` directories before `/bin`.

The dependencies of a build are first only resolved to ids. Their files are
put into the store (from a binary cache, `docs/cache.md`, or by building)
when the build runs, and the seed is fetched and assembled then.

A finished build is recorded in `index/<name>/<version>` as `<key> <id>`.
The key is the BLAKE3 hash of the canonical version record, the seed, the
revision of vug's build rules, and the ids of the dependencies in the build
root. A build whose key is already
in the index returns the recorded id without running.

## Command

```console
$ vug package build-package --root . --name hello
{
  "Built": true,
  "HostPath": "/home/user/.local/share/vug/store/fcyko5ygdr5mwm2q5nrfkfgwhn2aphu2",
  "Name": "hello",
  "Path": "/vug/store/fcyko5ygdr5mwm2q5nrfkfgwhn2aphu2",
  "Version": "2.12.2"
}
```

| Parameter | Meaning |
|---|---|
| `--root DIR` | Package tree holding `seed.toml` and `packages/`. |
| `--name NAME` | Package to build. |
| `--package-version VERSION` | Version to build. Default: the greatest version record. |
| `--data-dir DIR` | Holds `store/`, `index/`, and `seed/`. Default: `/vug` when `/vug/store` exists, else `$XDG_DATA_HOME/vug`, or `~/.local/share/vug`. |
| `--cache-dir DIR` | Holds `sources/`. Default: `/var/cache/vug` when `/vug/store` exists, else `$XDG_CACHE_HOME/vug`, or `~/.cache/vug`. |
| `--jobs N` | Parallel jobs for `make`. Default: 1. |
| `--binary-cache URL` | A binary cache to take outputs from (`docs/cache.md`). |

The build log goes to standard error. The flake package puts `bwrap`
(bubblewrap) on its `PATH`; a `vug` built with `cargo` needs `bwrap` on
`PATH`.

## Running a program

```console
$ vug package run-package --root . --name slang --program bin/slsh --arguments --version
slsh version 0.9.4-0; S-Lang version: 2.3.3
```

`run-package` builds the package when needed and then runs `--program` (a
path relative to the output) in a mount namespace that shows the host file
system and the store at `/vug/store`. It takes the parameters of
`build-package`, and `--arguments` takes every word after it. The exit status
is that of the program.

## Where the store lives

On the build host the store is the `store/` directory of the data directory.
A build sees its dependencies bound to their `/vug/store/<id>` paths, and
`run-package` sees the whole store at `/vug/store`. On a machine that runs a
system, `/vug/store` is a directory of the root file system
(`docs/system.md`).
