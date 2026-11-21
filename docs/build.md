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
   host file system, and only the environment variables `PATH`, `HOME`,
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
`/seed`, with `/seed/bin` also bound at `/bin`:

- `bin/busybox` and one link per busybox applet (`sh`, `tar`, `sed`, ...).
- `zig/`, the unpacked zig distribution.
- `bin/cc` and `bin/c++` run `zig cc` and `zig c++` for
  `x86_64-linux-musl`; `bin/ar`, `bin/ranlib`, and `bin/ld` run the matching
  zig tools; `bin/gcc` links to `cc`.

## Dependencies and the index

Each name in `dependencies.build` and `dependencies.run` is built first, at
its current version. The build root holds every dependency together with
what that dependency needs at run time, each at its store path, and `PATH`
lists their `bin` directories before `/bin`.

A finished build is recorded in `index/<name>/<version>` as `<key> <id>`.
The key is the BLAKE3 hash of the canonical version record, the seed, and
the ids of the dependencies in the build root. A build whose key is already
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
| `--data-dir DIR` | Holds `store/`, `index/`, and `seed/`. Default: `$XDG_DATA_HOME/vug`, or `~/.local/share/vug`. |
| `--cache-dir DIR` | Holds `sources/`. Default: `$XDG_CACHE_HOME/vug`, or `~/.cache/vug`. |
| `--jobs N` | Parallel jobs for `make`. Default: 1. |

The build log goes to standard error. `bwrap` (bubblewrap) must be on
`PATH`.

## Where the store lives

`/vug/store` exists only inside the build root. On the host the store is the
`store/` directory of the data directory, and each dependency is bound to its
`/vug/store/<id>` path in the sandbox.
