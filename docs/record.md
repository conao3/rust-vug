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
| `dependencies.build` | array of strings | yes | Package names needed while building. Sorted, no duplicates. |
| `dependencies.run` | array of strings | yes | Package names needed by the output at run time. Sorted, no duplicates. |
| `check.enabled` | boolean | yes | Whether the test suite runs. |
| `check.skip` | array of strings | yes | Test names to skip. Sorted, no duplicates. |

Unknown keys and unknown tables are errors.

### Paths in build options

Inside the strings of `configure-flags`, `make-flags`, `install-flags`, and
`setup-flags` (of every build kind that has them):

- `{out}` is replaced by the store path the package is installed to.
- `{dep:NAME}` is replaced by the store path of the dependency `NAME`, which
  must be listed in `dependencies.build` or `dependencies.run`.

```toml
configure-flags = [
  "--with-z={dep:zlib}",
  "--syslibdir={out}/lib",
]
```

### Build kind `autotools`

Runs `./configure --prefix=<output> <configure-flags>`, `make <make-flags>`,
`make check` (when `check.enabled`), `make install <install-flags>`.

| Key | Type | Default |
|---|---|---|
| `build.configure-flags` | array of strings, order kept | empty |
| `build.make-flags` | array of strings, order kept | empty |
| `build.install-flags` | array of strings, order kept | empty |

### Build kind `bootstrap-make`

Builds GNU make where no `make` exists yet: `./configure --prefix=<output>
<configure-flags>`, `./build.sh`, `./make install`.

| Key | Type | Default |
|---|---|---|
| `build.configure-flags` | array of strings, order kept | empty |

### Build kind `make`

For packages that ship a Makefile and no configure script. Runs `make
<make-flags>`, `make check <make-flags>` (when `check.enabled`), and `make
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

### Build kind `meson`

Runs `meson setup build --prefix=<output> --libdir=lib --buildtype=release
<setup-flags>`, `meson compile`, `meson test` (when `check.enabled`), and
`meson install`. The record needs `meson`, `python`, and a ninja
implementation (`samurai`) among its build dependencies.

| Key | Type | Default |
|---|---|---|
| `build.setup-flags` | array of strings, order kept | empty |

### Build kind `linux`

Builds a Linux kernel image for x86_64 with the seed compiler: `make
<defconfig>`, the `config` entries applied with the kernel's
`scripts/config`, `make olddefconfig`, `make bzImage`. The output holds
`boot/bzImage` and `boot/config`.

| Key | Type | Default |
|---|---|---|
| `build.defconfig` | string: a configuration target of the kernel (`tinyconfig`, `defconfig`) | required |
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

## Canonical form

Every record has exactly one byte representation. Two records with the same
content are the same bytes, so two changes that add the same version produce
identical files.

- UTF-8, `\n` line endings, exactly one trailing newline, no trailing spaces.
- No comments.
- Top-level keys first, then tables, each in the order of the tables above
  (`package.toml`: `name`, `license`, `[upstream]`; version files: `version`,
  `patches`, `[source]`, `[build]`, `[dependencies]`, `[check]`). Keys inside a
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
  document describes, and every record is canonical. It prints `Packages`,
  `Versions`, and `Errors` (each error has `Path` and `Message`) and exits 254
  when there is any error.

Whether a file is a `package.toml` or a version record is decided by its file
name: `package.toml` is a package record, any other `*.toml` is a version
record.
