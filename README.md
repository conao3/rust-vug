# vug

`vug` is a declarative package manager and system builder: packages,
services and systems are TOML records that carry data only, builds run in a
sandbox that starts from two upstream binaries (busybox and zig), and outputs
go to a content-addressed store. The records are in
[vug-packages](https://github.com/conao3/vug-packages); a system built from
them boots in QEMU and on EFI firmware, switches between generations, and
rebuilds itself on the machine.

| Document | Contents |
|---|---|
| [docs/record.md](docs/record.md) | Package records, the build kinds, the canonical form |
| [docs/build.md](docs/build.md) | Store paths, the seed, the compiler wrappers, the C++ runtime, how a build finds its dependencies |
| [docs/system.md](docs/system.md) | System and service records, the layout of a machine, switching and rolling back |
| [docs/cache.md](docs/cache.md) | Binary caches, signatures, garbage collection |

```console
$ vug version compare-versions --left 1.2.10 --right 1.2.9
{
  "Ordering": "Greater"
}
```

Package records live under `packages/<shard>/<name>/`: a `package.toml`, one
file per version in `versions/`, and patches in `patches/`. Three commands
check them (`--file` and `--root` are file system paths):

```console
$ vug record format-record --file packages/he/hello/versions/2.12.2.toml
{
  "Changed": false,
  "Record": "version = \"2.12.2\"\n..."
}
$ vug record check-record --file packages/he/hello/package.toml
$ vug record check-tree --root .
{
  "Errors": [],
  "Packages": 3,
  "Versions": 4
}
```

`format-record` prints the canonical byte form of a record (`--write` rewrites
the file). `check-record` exits 0 only for a valid, canonical file.
`check-tree` validates every record under `DIR/packages` and reports `Errors`.

```console
$ vug package build-package --root . --name hello --jobs 4
{
  "Built": true,
  "HostPath": "/home/user/.local/share/vug/store/fcyko5ygdr5mwm2q5nrfkfgwhn2aphu2",
  "Name": "hello",
  "Path": "/vug/store/fcyko5ygdr5mwm2q5nrfkfgwhn2aphu2",
  "Version": "2.12.2"
}
```

`build-package` builds a package and its dependencies in a sandbox that holds
only the seed tools named in `seed.toml`, and moves the output to its store
path (`docs/build.md`). The flake package puts `bwrap` (bubblewrap) on its `PATH`; a `vug` built
with `cargo` needs `bwrap` on `PATH`.

```console
$ vug package run-package --root . --name slang --program bin/slsh --arguments --version
slsh version 0.9.4-0; S-Lang version: 2.3.3
```

`run-package` runs a program of a package with the store visible at
`/vug/store`, which dynamically linked programs need.

```console
$ vug system build-system --root . --name minimal
$ vug system build-image --root . --name minimal --image minimal.img
```

`build-system` builds a system record (`docs/system.md`) into one store
path: packages, files below `/etc`, users, and the scripts that start it and
switch a running machine to it. `build-image` also writes a disk image: an
ext4 file system for a kernel that QEMU loads directly, or a GPT disk with an
EFI system partition when the record names a boot loader.

```console
# vug system switch-system --root /vug/tree --name minimal
# vug system delete-generations --keep 2
```

`switch-system` builds a system on the machine it runs on and makes it the
running generation; `delete-generations` drops old ones.

```console
$ vug store generate-key --output text > cache.key
$ vug store export-cache --directory cache --secret-key-file cache.key
# vug store collect-garbage
```

`export-cache` writes a binary cache, a directory to serve over HTTP, with a
signature per entry. A build given `--binary-cache URL --trusted-key KEY` (or
the same two values in `/etc/vug.toml`) takes outputs from it in place of
building them and refuses entries the key did not sign. `collect-garbage`
removes the store paths no generation reaches (`docs/cache.md`).

## Check

```console
$ nix develop --command make check
```

runs the format check, clippy, the Lean proof build, the leanassay audit, the
generated-file consistency check, and the tests. `nix build` produces
`./result/bin/vug`.
