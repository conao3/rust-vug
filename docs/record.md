# Package records

A package is described by declarative TOML records. Records carry data only:
there is no field that holds a shell command. Everything a build does comes
from the build kind named in the record.

## Tree layout

```text
packages/
  he/hello/
    package.toml
    versions/
      2.12.1.toml
      2.12.2.toml
    patches/
      2.12.2-fix-tests.patch
```

- A package lives in `packages/<shard>/<name>/`.
- `<shard>` is the first two characters of the name. A one-character name uses
  that character followed by `_` (`packages/r_/r/`).
- Each version is one file, `versions/<version>.toml`.
- Patches live in `patches/` and are named `<version>-<slug>.patch`.
- There is no file that lists packages and no file that names the current
  version. The current version of a package is the greatest file in
  `versions/` under `vug version compare-versions`.

## Names and versions

- A package name matches `[a-z0-9][a-z0-9+._-]*` and is at most 64 characters.
- A version matches `[A-Za-z0-9][A-Za-z0-9+._~-]*` and is at most 64
  characters.
- Neither may be `.` or `..`, and neither may end with `.toml`.

## `package.toml`

```toml
name = "hello"
license = "GPL-3.0-or-later"

[upstream]
homepage = "https://www.gnu.org/software/hello/"
sources = [
  "https://ftp.gnu.org/gnu/hello/",
]
```

| Key | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | Must equal the directory name. |
| `license` | string | yes | SPDX license expression. |
| `upstream.homepage` | string | no | Project home page. |
| `upstream.sources` | array of strings | yes, non-empty | URL prefixes that source URLs of this package may start with. Each is an `https://` URL ending with `/`. Sorted, no duplicates. |

## `versions/<version>.toml`

```toml
version = "2.12.2"

[source]
url = "https://ftp.gnu.org/gnu/hello/hello-2.12.2.tar.gz"
sha256 = "5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab"

[build]
kind = "autotools"
configure-flags = [
  "--disable-nls",
]

[dependencies]
build = [
  "make",
]
run = []

[check]
enabled = true
skip = []
```

| Key | Type | Required | Meaning |
|---|---|---|---|
| `version` | string | yes | Must equal the file name without `.toml`. |
| `source.url` | string | yes | `https://` URL. Must start with one of `upstream.sources`. |
| `source.sha256` | string | yes | 64 lowercase hex characters: SHA-256 of the fetched file. |
| `source.strip-components` | integer | no (default 1) | Leading path components removed when the archive is unpacked. |
| `patches` | array of strings | no (default empty) | File names in `patches/`, applied in order with `-p1`. Order is kept. |
| `build.kind` | string | yes | One of the build kinds below. |
| `build.*` | | | Options of that kind. Unknown options are rejected. |
| `dependencies.build` | array of strings | yes | Dependency selectors needed while building. Sorted, no duplicate selections. |
| `dependencies.run` | array of strings | yes | Dependency selectors needed by the implicit `out` output at run time. Sorted, no duplicate selections. Empty when declaring `[outputs]`. |
| `dependencies.expose` | array of strings | no | Dependency selectors passed to builds against this package, such as the headers its own headers include or the pkg-config files its pkg-config files require. Sorted, no duplicate selections. Omitted when empty. |
| `check.enabled` | boolean | yes | Whether the test suite runs. |
| `check.skip` | array of strings | yes | Test names to skip when `check.enabled` is true. Sorted, no duplicates. A nonempty list requires a build kind that supports exclusions. |
| `check.target` | string | no | Make target for enabled checks in `autotools` and `make`. Defaults to `"check"`. |
| `check.private-cxx-runtime` | boolean | no | `true` lets the output hold a statically linked C++ runtime (`build.md`, "The C++ runtime"): the runtime package itself and the tools it is built with. Left out when false. |

Unknown keys and unknown tables are errors.

A source file named `*.tar.lz` is decoded from lzip before tar extraction.
A source URL ending in `.zip` is extracted by vug after SHA-256 verification,
using `source.strip-components` and preserving Unix file permissions. vug checks
entry paths and symlink targets before extraction. Symlink targets may start
with parent components that stay within the stripped tree. Patches are applied
in the build sandbox after extraction.
`source.sha256` covers the downloaded, compressed file.

### Outputs and dependency selectors

A bare package name selects `out`. Use `package:output` to select an output
explicitly in `dependencies.build`, `dependencies.run`, `dependencies.expose`,
or an output's `run` list. For example, a consumer can list both `llvm:dev`
and `llvm:lib` in `dependencies.build`. Within an output's `run` list,
`:output` selects another output of the same build.

A version record with output tables must declare `[outputs.out]` and set
`dependencies.run = []`. Use `dependencies.build` for shared build inputs
and each output's `run` list for its own runtime dependencies. Output names
follow the package-name rules. A version without output tables has one
implicit output named `out`.

```toml
[outputs.out]
run = []

[outputs.lib]
paths = ["lib/*.so.*"]
run = ["zlib"]

[outputs.dev]
paths = ["include/**", "lib/*.a", "lib/example.so"]
run = [":lib"]
```

`paths` lists exact paths or glob patterns relative to the staging tree.
`*` matches zero or more characters within a path component, `?` matches
one character within a component, and `**` as an entire component matches
zero or more components. Other characters match literally. All components
must be nonempty and stay inside the staging tree. The partition assigns
unmatched files to `out`. Declarations that can assign the same path to two
outputs are invalid.

Both arrays default to empty. Canonical formatting writes `paths` when
nonempty and always writes `run`. Both arrays and the output tables are
sorted by byte order. Record validation checks local output names and cycles.
Tree validation also resolves external selectors to the current version of
each dependency and checks for unknown outputs and dependency cycles.
The builder rejects patterns that match no staged path. See
[split outputs](build.md#split-outputs) for relocation and indexing.

### Selecting tests to skip

| Build kind | Test name and exclusion |
|---|---|
| `autotools` | An exact word from an Automake `TESTS` list, such as `tests/example.sh` or `test-float-h`, as written in that Makefile. Names use ASCII letters, digits, or `._/+-`. vug filters `TESTS` and the corresponding `check_PROGRAMS` entries in configured Automake Makefiles for `make check`. Leave those variables to the harness when using exclusions. [Automake test suites](https://www.gnu.org/software/automake/manual/html_node/Tests.html), [GNU make text functions](https://www.gnu.org/software/make/manual/html_node/Text-Functions.html). |
| `cmake` | An exact CTest name. vug escapes and anchors the names in the regular expression passed to `ctest -E`. CTest can still add fixture setup and cleanup tests required by the remaining tests. [CTest options](https://cmake.org/cmake/help/latest/manual/ctest.1.html). |
| `meson` | An exact test name from `meson introspect --tests build`. vug passes the remaining names to `meson test`. Tests with the same name in different subprojects are excluded together. [Meson introspection](https://mesonbuild.com/IDE-integration.html), [Meson test command](https://mesonbuild.com/Commands.html#test). |
| `cargo` | A full libtest name, including any module path. A nonempty list opts into an offline, locked `cargo test --release` for the manifest under `build.path`, with `--exact` and one `--skip` per name. The test uses Cargo's default features. `build.flags` are passed to `cargo install`. [Cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html), [Rust test filters](https://doc.rust-lang.org/rustc/tests/index.html#filters). |

`make` and the other build kinds require an empty list. A plain Makefile
defines its own `check` target and test selection. [GNU make standard
targets](https://www.gnu.org/software/make/manual/html_node/Standard-Targets.html).

### Paths in build options

Inside the strings of `configure-flags`, `make-flags`, `install-flags`, and
`setup-flags` (of every build kind that has them):

- `{out}` is replaced by the store path the package is installed to.
- `{dep:NAME}` is replaced by the store path of the dependency `NAME`, which
  must be listed in `dependencies.build` or `dependencies.run`. Use
  `{dep:NAME:OUTPUT}` for an explicitly listed selector.

```toml
configure-flags = [
  "--with-z={dep:zlib}",
  "--syslibdir={out}/lib",
]
```

### Build kind `autotools`

Set `build.path` to the relative source directory containing the build definition. It defaults to `"."`.

Runs `autoreconf -fi` when `build.autoreconf` is true, then
`./configure --prefix=<output> <configure-flags>`, `make <make-flags>`,
`make <check.target>` (when `check.enabled`),
and `make <install-targets> <make-flags> <install-flags> DESTDIR=/build/dest`.
The install targets must honor `DESTDIR`. Files staged under the configured
prefix go into the output. Files staged outside the prefix keep their paths
relative to the staged root under the output's `.vug/root/`.

| Key | Type | Default |
|---|---|---|
| `build.autoreconf` | boolean | false, omitted from the canonical record. When true, `dependencies.build` must include `autoconf`, `automake`, and `libtool`. |
| `build.configure-command` | array of strings, order kept | empty, which means `./configure --prefix=<output>` |
| `build.configure-flags` | array of strings, order kept | empty |
| `build.make-flags` | array of strings, order kept | empty |
| `build.install-flags` | array of strings, order kept | empty |
| `build.install-targets` | array of strings, order kept | empty, which means `install` |

### Build kind `bootstrap-make`

Builds GNU make where no `make` exists yet: `./configure --prefix=<output>
<configure-flags>`, `./build.sh`, `./make install`.

| Key | Type | Default |
|---|---|---|
| `build.configure-flags` | array of strings, order kept | empty |

### Build kind `make`

For packages that ship a Makefile and no configure script. Runs `make
<make-flags>`, `make <check.target> <make-flags>` (when `check.enabled`), and `make
install <make-flags> <install-flags>`. The install location is passed as a
flag, the way the package's Makefile names it:

```toml
[build]
kind = "make"
make-flags = [
  "PREFIX={out}",
]
```

| Key | Type | Default |
|---|---|---|
| `build.make-flags` | array of strings, order kept | empty |
| `build.install-flags` | array of strings, order kept | empty |
| `build.build-targets` | array of strings, order kept: targets of the first `make` | empty (the default target) |
| `build.install-targets` | array of strings, order kept: targets of the last `make` | `install` |

### Build kind `cargo`

Builds a Rust package with `cargo install --path <path> --root <output>
--offline --locked --no-track <flags>`. The record needs `rust` among its
build dependencies.

The build has no network, so the crates come first: the source is unpacked,
its `Cargo.lock` is read, and every crate the lock file names is fetched
from `https://static.crates.io/crates/<name>/<name>-<version>.crate` and
checked against the checksum in the lock file. The SHA-256 of the source
therefore fixes every crate. A lock file entry from another registry or from
git is an error. In the build the crates are a vendor directory that
replaces crates.io; the package's own `.cargo` directory is removed; the
linker is `cc`, and programs link against the C library of the build
(`-C target-feature=-crt-static`), so the record lists `musl` and
`rust-libunwind` in `dependencies.run`.

| Key | Type | Default |
|---|---|---|
| `build.path` | string | `"."`: the directory of the package inside the source |
| `build.flags` | array of strings, order kept | empty |

### Build kind `go`

Use `dependencies.build = ["go"]` and declare the main packages and installed executable names.

```toml
[build]
kind = "go"
targets = [
  { path = ".", bin = "example" },
  { path = "./cmd/helper", bin = "helper" },
]
```

Builds use the root Go module, disable cgo, and install executables in `bin/`.
A source with a `vendor/` tree uses Go vendor mode. Otherwise vug fetches
the Go module files named by `go.sum` from `https://proxy.golang.org`.
Before building, it checks module manifests and ZIPs against their Go checksums
and resolves dependencies for the executable targets and enabled tests.
Missing files or checksums and mismatches stop dependency preparation.
The build and tests run offline with the prepared module cache.
Workspace discovery and toolchain downloads are disabled.

Set `check.enabled = true` to run `go test ./...` under the same constraints.
Use an empty `check.skip` list. List runtime tools and data separately in `dependencies.run`.

### Build kind `node`

```toml
[build]
kind = "node"
build-script = true

[dependencies]
build = ["npm"]
run = ["nodejs"]

[check]
enabled = false
skip = []
```

Use a source archive with `package.json` and an npm `package-lock.json`
at the project root. The lockfile must be version 3 and describe registry
tarballs with SHA-512 integrity. Git, file and workspace dependencies
require a different packaging path. vug verifies the tarballs before
populating a fresh npm cache inside the network-disabled build sandbox.
Optional dependencies are selected for Linux, the target CPU and musl.

`build.build-script` defaults to false. With it enabled, vug runs the
manifest's `build` script. Set `check.enabled` to run `npm test` and
use an empty `check.skip` list. npm disables dependency lifecycle scripts
and the implicit pre/post hooks around the explicit build and test scripts.
The final install includes only production dependencies. The project lives
under `lib/node_modules/<name>`; manifest bin entries use launchers in
`bin/` that invoke the declared Node executable.

For a bundled musl executable with an interpreter outside the store, add
its project-relative path to `build.musl-executables`. List `patchelf`
in `dependencies.build` and `musl` in `dependencies.run`. vug sets the
listed interpreter paths to the musl loader in the store after each install.

### Build kind `perl`

Build a pinned CPAN source archive with the `perl` interpreter from
`dependencies.run`. Set `driver = "make-maker"` for `Makefile.PL` and add
`make` to `dependencies.build`. Set `driver = "module-build"` for `Build.PL`
and provide `Module::Build` as a build dependency when the archive needs it.
Both drivers install under the output's `lib/perl5`, including the
interpreter's architecture subdirectory for XS extensions.

Put configure, build and test tools in `dependencies.build`. Put imported
modules and shared libraries in `dependencies.run`. When CPAN metadata is
present, required modules and versions are checked for the applicable phases.
The build uses the declared dependencies' Perl libraries and rejects
incompatible ABI markers. Installed Perl scripts use the selected interpreter
and a fixed `PERL5LIB` containing the output and its runtime dependency closure.

With `check.enabled = true`, the driver runs the upstream test suite and
loads each module named in the optional `build.modules` array from the
installed tree. Module names use Perl syntax, for example `"Try::Tiny"`.
Set `check.skip` to project-relative test paths such as `"t/network.t"`
to exclude individual files from the driver’s `t/*.t` suite.

### Build kind `python`

`kind = "python"` builds a wheel from the patched source tree using
`python -m build --wheel --no-isolation` in the networkless build sandbox.
Sources with only `setup.py` use the frontend's setuptools compatibility
backend. Declare `python` in `dependencies.run` and `python-build`,
`python-installer`, and the backend in `dependencies.build`. The frontend
checks the declared backend and its dynamic build requirements. Declare
import dependencies and native runtime libraries in `dependencies.run`.
The build checks the installed wheel's active distribution requirements
against the runtime closure.

The build validates the wheel tag and archive paths and installs with
`installer` under `lib/pythonX.Y/site-packages`. Shared objects there
must load with the selected Python and runtime libraries. Wheel scripts
use the selected Python and a fixed `PYTHONPATH`, with the package's own
modules first, followed by direct and transitive runtime dependencies.

With `check.enabled = true`, `build.imports` lists modules to import
and `build.native-imports` lists modules that must load as native
extensions. These checks run in a new Python process with the installed
modules and runtime closure. Set `build.pytest = true` and declare
`pytest` and its plugins in `dependencies.build` to run the source
suite against the installed wheel. Each field is optional.

### Build kind `commands`

Runs the listed commands in order in the unpacked source. Each command is an
array of words: the program and its arguments, with no shell in between. It
is for the packages no other kind fits, such as a binary distribution that
needs a library linked or files moved.

| Key | Type | Default |
|---|---|---|
| `build.commands` | array of arrays of strings, order kept | required, non-empty |

### Build kind `cmake`

Runs `cmake -S <path> -B build -G "Unix Makefiles"
-DCMAKE_INSTALL_PREFIX=<output> -DCMAKE_INSTALL_LIBDIR=lib
-DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_RPATH=<output>/lib
<setup-flags>`, `cmake --build`, `ctest`
(when `check.enabled`), and `cmake --install`. The record needs `cmake` and
`make` among its build dependencies. cmake finds the headers and libraries of
the dependencies through `CMAKE_PREFIX_PATH`, which every build sets to the
store paths of the dependencies the record lists.

| Key | Type | Default |
|---|---|---|
| `build.path` | string | `"."`: the directory inside the source that holds the top `CMakeLists.txt` |
| `build.setup-flags` | array of strings, order kept | empty |

### Build kind `meson`

Set `build.path` to the relative source directory containing the build definition. It defaults to `"."`.

Runs `meson setup build --prefix=<output> --libdir=lib --buildtype=release
<setup-flags>`, `meson compile`, `meson test` (when `check.enabled`), and
`meson install`. The record needs `meson`, `python`, and a ninja
implementation (`samurai`) among its build dependencies.

Files that the package installs outside its prefix (default configuration
under `/etc`, state directories under `/var`) are kept in the output under
`.vug/root/`, at the path they were meant for.

meson removes from an installed program every library search path it did not
add itself. The build therefore also gives meson the run-time library paths
of the dependencies through `LDFLAGS`, which meson keeps.

| Key | Type | Default |
|---|---|---|
| `build.setup-flags` | array of strings, order kept | empty |

### Build kind `linux`

Builds a Linux kernel image for x86_64 with the seed compiler: `make
<defconfig>`, the `config` entries applied with the kernel's
`scripts/config`, `make olddefconfig`, `make bzImage`. The output holds
`boot/bzImage` and `boot/config`.

With `build.modules = true`, add `kmod` as a build dependency and enable
`MODULES=y`. The build also runs `make modules`, `make modules_install`
and `depmod`. The output contains `lib/modules/<kernelrelease>/` with the
installed modules and their indexes. The kernel release includes the build key
so changes to the build inputs produce a distinct release.

| Key | Type | Default |
|---|---|---|
| `build.defconfig` | string: a configuration target of the kernel (`tinyconfig`, `defconfig`) | required |
| `build.modules` | boolean: build and index kernel modules | false |
| `build.config` | array of strings `NAME=value`, order kept. `y` enables, `n` disables, `m` builds a module, anything else is set as the value | empty |

The programs that run during the build (`objtool` and the other host tools)
are compiled against the dependencies; the kernel itself is not. The record
needs `bash`, `bison`, `flex`, `elfutils`, and `musl` among its build
dependencies.

### Build kind `prebuilt`

Unpacks the source archive and copies files into the output.

| Key | Type | Default |
|---|---|---|
| `build.install` | array of inline tables `{ from = "...", to = "..." }`, order kept | required, non-empty |

`from` is a path relative to the unpacked source. `to` is a path relative to
the output. Neither may be absolute or contain a `..` component.

## Links

```toml
[links]
"lib/libtinfo.so" = "libtinfow.so"
```

After the steps of the build kind, each key becomes a symbolic link in the
output, at that path relative to the output, pointing to the value. This
gives a file a second name that other packages look for. The table is the
last one of a version record and is left out when empty.

## Wrappers

```toml
[wrappers]
"bin/rustc" = ["{dep:musl}/lib/ld-musl-x86_64.so.1", "--library-path", "{out}/lib", "{out}/libexec/rustc"]
```

After the steps of the build kind and the links, each key becomes an
executable shell script in the output, at that path, that replaces itself by
the listed command followed by its own arguments. `{out}` and `{dep:NAME}`
are replaced as in flags. This starts a program that was built for a fixed
loader path through the loader of the C library package. The table follows
`[links]` and is left out when empty.

## Canonical form

Every record has exactly one byte representation. Two records with the same
content are the same bytes, so two changes that add the same version produce
identical files.

- UTF-8, `\n` line endings, exactly one trailing newline, no trailing spaces.
- No comments.
- Top-level keys first, then tables, each in the order of the tables above
  (`package.toml`: `name`, `license`, `[upstream]`; version files: `version`,
  `patches`, `[source]`, `[build]`, `[dependencies]`, `[outputs.*]`,
  `[check]`, `[links]`,
  `[wrappers]`). Keys inside a
  table follow the order of the tables above; `build.kind` comes first in
  `[build]`.
- One blank line before each table header. No other blank lines.
- Keys with their default value are omitted (`source.strip-components = 1`,
  empty `patches`, empty kind options).
- Strings are basic strings in double quotes. Only `\"`, `\\`, and `\uXXXX`
  for control characters are escaped.
- An empty array is `[]`. A non-empty array has one element per line, indented
  by two spaces, each followed by a comma.
- Inline tables are written `{ from = "a", to = "b" }`.
- Arrays marked "sorted" are sorted by byte order.

## Commands

```console
$ vug record format-record --file packages/he/hello/versions/2.12.2.toml
{
  "Changed": false,
  "Record": "version = \"2.12.2\"\n..."
}
```

- `vug record format-record --file PATH [--write]` parses a record, validates
  it, and prints the canonical form. With `--write` it rewrites the file when
  the canonical form differs. `Changed` tells whether the file differs from
  its canonical form.
- `vug record check-record --file PATH` exits 0 when the file is valid and
  canonical. It exits 254 and reports `Errors` otherwise.
- `vug record check-tree --root DIR` checks every record under
  `DIR/packages`: path and name agree, version and file name agree, every
  `source.url` starts with one of the package's `upstream.sources`, every
  listed patch exists, every file under `packages/` is one of the files this
  document describes, and every record is canonical. It also reads every
  `systems/*.toml` with its service records (`docs/system.md`). It prints
  `Packages`, `Versions`, `Systems`, and `Errors` (each error has `Path` and
  `Message`) and exits 254 when there is any error.

Whether a file is a `package.toml` or a version record is decided by its file
name: `package.toml` is a package record, any other `*.toml` is a version
record.
