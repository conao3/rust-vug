# User environment records

A home record lives at `homes/<name>.toml`. It can select reusable records from `home-fragments/<name>.toml` in the same repository. Each record has a required `name` matching its filename without the `.toml` extension. Home names, fragment names, package names, and logical secret names follow the [package-name syntax](record.md#names-and-versions).

```toml
name = "desktop"
fragments = ["shell"]
packages = ["hello"]

[environment]
EDITOR = "vi"

[[file]]
path = ".config/example/settings.ini"
content = "theme=light\n"

[[unit]]
name = "example.service"
enable = ["default.target"]
secrets = ["example-key"]
content = """
[Service]
Type=oneshot
ExecStart={pkg:hello}/bin/hello
"""

[[secret]]
name = "example-key"
```

A fragment uses the same fields except `fragments`, which belongs to the home record. The fields below are optional and default to empty collections.

- `fragments` lists selected fragment names.
- `packages` lists package names.
- `environment` maps ASCII identifiers matching `[A-Za-z_][A-Za-z0-9_]*` to literal strings. Values are preserved as written, including shell syntax, and must be free of NUL bytes.
- Each `[[file]]` sets a HOME-relative `path` and either literal `content` or a `package` and `source` pair. The pair selects a relative path inside a package output. Both paths must have nonempty components other than `.` or `..` and must be free of NUL bytes. Home file paths must stay clear of `.local/state/vug` and `.config/systemd/user`, including their parents and descendants.
- Each `[[unit]]` sets a systemd unit `name` and its public `content` as a NUL-free string. Optional `enable` and `secrets` list target unit names and logical secret names, respectively. The example preserves the `{pkg:hello}` reference in the unit text.
- Each `[[secret]]` declares a logical `name`. The record carries the name alone.

Composition takes the union of the home and its selected fragments. Packages, files, variables, units, and secrets are keyed by package name, destination path, variable name, unit name, and secret name, respectively. Identical definitions collapse to a single entry. Different definitions for one key and file paths nested inside other declared file paths are errors, including across records. Package-backed files must refer to selected packages, and units must refer to declared secrets. These references can be satisfied by any record in the union. The schema rejects unknown keys in each record and in its entries.

## Commands

- `vug home format-home --file PATH [--write]` validates and formats one home record. It retains its fragment selections.
- `vug home format-fragment --file PATH [--write]` validates and formats one fragment.
- `vug home compose-home --root DIR --name NAME` reads the named home and its fragments and prints a single canonical TOML record containing their combined entries.

Formatting sorts names and paths, removes repeated set members and identical definitions, and omits empty collections. With `--output text` each command prints TOML. The default JSON output includes the TOML string under `Record`. Formatting also reports `Changed` and rewrites the input only with `--write`. Input errors exit with status 254.
