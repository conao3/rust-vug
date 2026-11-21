# vug

`vug` is a declarative package manager and system builder: packages are TOML
records, builds run in a sandbox, and outputs go to a content-addressed store.
This repository currently contains the project scaffolding and one command,
which compares two version strings.

```console
$ vug version compare-versions --left 1.2.10 --right 1.2.9
{
  "Ordering": "Greater"
}
```

## Check

```console
$ nix develop --command make check
```

runs the format check, clippy, the Lean proof build, the leanassay audit, the
generated-file consistency check, and the tests. `nix build` produces
`./result/bin/vug`.
