# vug

`vug` is a declarative package manager and system builder: packages are TOML
records, builds run in a sandbox, and outputs go to a content-addressed store.
This repository currently contains the project scaffolding, a command that
compares two version strings, and the package-record commands of
`docs/record.md`.

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
switch a running machine to it. `build-image` also writes a disk image that
boots it in QEMU.

## Check

```console
$ nix develop --command make check
```

runs the format check, clippy, the Lean proof build, the leanassay audit, the
generated-file consistency check, and the tests. `nix build` produces
`./result/bin/vug`.
