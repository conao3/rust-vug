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
2. Decode a `.tar.lz` source into a temporary tar file with vug's built-in
   lzip decoder. Create the build root: the seed tools, each dependency at its store path,
   the source archive, the patches, and an empty output directory at the
   placeholder path.
3. Run the build inside `bwrap --unshare-all --hostname vug --clearenv`: no network, no
   host file system, a root that is read-only outside the build directory,
   `/tmp`, and the output, and only the environment variables `PATH`, `HOME`,
   `SOURCE_DATE_EPOCH`, `build_alias`, `ZIG_GLOBAL_CACHE_DIR`, and `ZIG_LOCAL_CACHE_DIR`.
4. Unpack with `tar -xf <archive> --strip-components=<n>` and apply the
   patches with `patch -p1`. An incomplete or rejected hunk fails the build. A source whose file name has no `.tar` in it
   and does not end in `.tgz`, `.txz`, or `.tbz2` is not an archive: it is
   copied into the source directory under its name.
5. Run the steps of the build kind (`docs/record.md`). For an autotools
   record with `autoreconf = true`, run `autoreconf -fi` in the patched
   source directory before the configure command.
6. Copy the version record to `.vug/record.toml` in the output.
7. Hash the output, replace the placeholder, and move the output to its
   store path.

`build_alias` is `x86_64-unknown-linux-musl`. A configure script made by
autoconf takes it as the system it builds on and for. Its own guess is
`...-linux-gnu`, because the guess looks for a mark that the C library's
`stdarg.h` leaves and the compiler's own `stdarg.h` comes first; packages
then choose code paths for another C library.

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
    put its own copy of such a header first. The same translation applies
    to directories in `VUG_CFLAGS`, after the package's own directories.
  - `-march=x86-64` becomes `-march=x86_64` (and `-v2` to `-v4` likewise).
  - `-m16` also enables the 16-bit mode feature, which zig switches off.
  - `-mno-sse` also disables SSE2 and later, which zig leaves on.
  - `-Wl,-R<dir>` becomes `-Wl,-rpath,<dir>` and `-Wl,-h<name>` becomes
    `-Wl,-soname,<name>`; `-stats`, `-lc`, `-shared`,
    `--allow-multiple-definition`, and `-Map` with its file inside `-Wl,`
    are dropped.
  - `-Wl,-retain-symbols-file` becomes a linker version script that lists
    the file's symbols as global and marks the other symbols local, as
    the seed's lld does.
  - Linker lines printed by `-v` omit Zig's temporary cache inputs and
    internal links. They describe the packaged C++ runtime with `-L` and
    `-l` flags so Libtool and CMake can reuse those dependencies.
  - `-S` together with a dependency file (`-MD`, `-MMD`) runs zig twice,
    because zig writes no assembly output for that combination.
  - input files ending in `.oo` and relocatable ELF objects ending in
    `.so` or `.os` are passed through symlinks ending in `.o`, which
    zig recognizes as object files.
  - input from standard input (`-`) without `-x` is given `-x c` (`-x c++`
    for `c++`); without it zig does not search the C library headers.
  - a link whose inputs are only libraries gets an empty source file as
    well; zig cannot link without an object of its own.
  - `-fno-sanitize=undefined` comes before the arguments of the package:
    without an optimization level zig compiles with the undefined-behavior
    sanitizer, and a program then aborts where the same code built by
    another compiler runs.
  - `-UNDEBUG` comes before the arguments of the package: with an
    optimization level zig defines `NDEBUG`, which switches every `assert`
    off; a package that wants that passes `-DNDEBUG` itself.
  - preprocessed output of standard input (`-E -`) names the input
    `<stdin>` in its line markers, as other compilers do; zig names a
    temporary file.
  - `-fsyntax-only` becomes a compile into a temporary object, and with it
    `-x c-header` and `-x c++-header` become `-x c` and `-x c++`: zig fails
    on `-fsyntax-only` after the check itself has passed.
  - `-nostdlib` (without `-nostdinc`) keeps the headers of the C library on
    the search path; zig drops them.
  - `-Wl,-rpath` followed by the directory as its own argument is joined
    into `-Wl,-rpath,<dir>`.
  - `-Wl,-init,<function>` and `-Wl,-fini,<function>`, which zig rejects,
    become a small source file that calls the function from a constructor
    or a destructor.
  - an unused command-line argument and a use of `__DATE__` or `__TIME__`
    are never errors (zig makes the second one an error at `-O3`; with
    `SOURCE_DATE_EPOCH` set the macros are reproducible).
- `bin/ar`, `bin/ranlib`, and `bin/ld` run the matching
  zig tools; `bin/gcc` links to `cc`.
- `bin/cpp` runs `cc -E`, on standard input when no file is named.

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
| `include/` | `-isystem <path>/include` in `VUG_CFLAGS`, translated by the wrappers to `-I<path>/include`. The compiler searches the package's own directories, then its dependencies, then the seed headers. |
| `lib/` | `-L<path>/lib`, and `-Wl,-rpath,<path>/lib` when the dependency is needed at run time (it is in `dependencies.run`, or a run-time dependency of one that is). |
| `lib/pkgconfig/`, `share/pkgconfig/` | Added to `PKG_CONFIG_PATH`. |
| `bin/pkgconf` | `PKG_CONFIG` is set to it. |
| `lib/python*/site-packages/` | Added to `PYTHONPATH`. |
| `share/aclocal/` | Added to `ACLOCAL_PATH`, where `aclocal` and `autoreconf` find the macros of libtool, pkgconf, and others. |

Two flags are always added when linking: `-Wl,-rpath,<output>/lib`, so a
program finds the libraries of its own package, and
`-Wl,--undefined-version`, so a linker version script may name symbols that
the library does not define (the seed linker rejects that by default, and
many packages rely on it).

A package that does not depend on a C library with a loader is linked
statically against the C library bundled with the seed compiler.

## The C++ runtime

`zig c++` links libc++ statically into every program and shared library it
produces. Two objects of one process then hold separate copies of the
runtime: `std::error_code` values compare unequal across them, and so do
the type identities that exceptions and `dynamic_cast` rely on.

A package whose C++ code meets other C++ code at run time therefore lists
the runtime package in `dependencies.run`. The runtime package is the one
whose output has `include/c++/v1/__config_site`: libc++, libc++abi and
libunwind as shared libraries, of the LLVM release the seed's zig bundles.
When it is among the dependencies of a build, `VUG_LIBCXX` holds its store
path and the wrappers change as follows:

- `c++` passes `-nostdlib++` and `-I$VUG_LIBCXX/include/c++/v1` (the
  second not when the call has `-nostdinc++`), and at a link names
  `libc++.so.1`, `libc++abi.so.1` and `libunwind.so.1` of that package by
  path, with its `lib` as a run path. For shared links with `-nostdlib`,
  it still adds the packaged C++ runtime and records `libc++.so.1` as a
  direct dependency. An explicit `-nostdlib++` suppresses automatic
  C++ runtime linking.
- `cc` and `c++` translate explicit `-lc++`, `-lstdc++`, `-lc++abi` and
  `-lunwind` to those packaged libraries.

The builder rejects an installation with no payload files, including installations that only contain vug metadata. Files staged outside the prefix under `.vug/root` count as payload.

After the install step the build fails when a file of the output is an
ELF object holding a statically linked C++ runtime, unless the record sets
`check.private-cxx-runtime`.

## Dependencies and the index

Each name in `dependencies.build` and `dependencies.run` is built first, at
its current version. What a dependency exposes (`dependencies.expose` of the
dependency itself and of everything it needs at run time) is in the build
root as well, and stays out of the run-time closure of the output. The build root holds every dependency together with
what that dependency needs at run time, each at its store path, and `PATH`
lists their `bin` directories before `/bin`.

The dependencies of a build are first only resolved to ids. Their files are
put into the store (from a binary cache, `docs/cache.md`, or by building)
when the build runs, and the seed is fetched and assembled then.

A single-output build is recorded in `index/<name>/<version>` as
`<key> <id>`. When the key matches and the requested output is available,
the builder returns the recorded ID.

A record with neither output tables nor an explicit output selector uses the
BLAKE3 hash of the concatenated canonical record, seed ID, build rules
revision, and sorted IDs of the dependencies visible in the build root.

Output declarations and explicit selectors select a versioned key format.
It uses the BLAKE3 derive-key context `vug build key for selected outputs v1`.
Every field has an unsigned 64-bit little-endian byte length followed by
its UTF-8 bytes. The fields are the canonical record, seed ID, build rules
revision, the decimal count of visible dependency IDs, the sorted IDs, and
the sorted dependency bindings. Each binding has four fields: the record
field name (for example, `dependencies.build` or `outputs.dev.run`), the
external dependency package name, the selected output name, and its resolved
ID. Same-build output edges are represented in the canonical record.
Both formats encode the first 20 hash bytes in base32. A single-output build
can use an explicit `package:out` selector for a single-output dependency.

## Split outputs

A record with `[outputs]` installs once into the usual staging prefix. Each
path belongs to one output. A matching directory assigns its descendants
too. An unmatched pattern or a path assigned to multiple outputs fails the
build. The builder assigns unclaimed paths to `out` and reserves `.vug` for
its metadata.

The builder finalizes outputs in the order of their declared local runtime
dependencies. Relative links within an output keep their original targets.
Links to other outputs and embedded absolute staging paths use the target
output's store path. The builder expands staging-based path variables in
pkg-config files and relocates ELF search paths using the partition. It
rejects ambiguous embedded paths, references outside the declared runtime
closure, and unresolved ELF interpreters and needed libraries. Build flags
supply space for the relocated ELF search paths. An ELF with insufficient
space fails the build.

The build root includes the selected dependencies, their runtime closures
and their exposed dependencies. An output declared as a runtime dependency in
any output table is available during the shared build. For each finished ELF,
the builder removes automatically supplied search paths outside that ELF's
output closure.

The tree hash includes each output's `.vug/output` name marker and uses the
placeholder for that output's own path. `out` also holds `.vug/record.toml`.
Each output lives at `store/<id>`. After installing the whole set, the builder
atomically publishes a JSON manifest at `index-outputs/<name>/<version>`.
The manifest has `format` (currently `1`), `key`, and `outputs` fields.
`outputs` maps each output name to its `id` and sorted list of direct
runtime dependency IDs in `run`. The builder checks the manifest against the
record and the resolved runtime dependencies. Concurrent publications for
the same key must agree on the whole manifest.

## Command

Pass `--name NAME:OUTPUT` to select an output with `build-package` or
`run-package`. A bare name selects `out`.

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
| `--data-dir DIR` | Holds `store/`, `index/`, `index-outputs/`, and `seed/`. Default: `/vug` when `/vug/store` exists, else `$XDG_DATA_HOME/vug`, or `~/.local/share/vug`. |
| `--cache-dir DIR` | Holds `sources/`. Default: `/var/cache/vug` when `/vug/store` exists, else `$XDG_CACHE_HOME/vug`, or `~/.cache/vug`. |
| `--source-cache-dir DIR` | Read build inputs exclusively from SHA-256-named files in DIR. |
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


### Verified source directory

Pass `--source-cache-dir DIR` to `vug package build-package` to use a prepared source directory. Before resolving builds, vug checks the SHA-256 of the seed inputs, the archives for the requested package and its recursive dependencies, and the blobs matching their patch files. A missing or corrupt file stops the command. The offline build reads archives and patch bytes from this directory. Keep it read-only for the duration of the build. This option disables binary cache access and bypasses the ordinary download cache.

Cargo registry crates must also exist in DIR under their lockfile SHA-256. Vendored Go modules can come from the verified source archive. A Go build that needs the public module proxy fails in this mode. Node builds fail when the lockfile needs registry tarballs. Python and Perl use dependency packages resolved from the same source directory. Unavailable language dependencies fail during build preparation without a network fetch. Build and test commands continue to run in the network-isolated sandbox.
