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
- Each `[[file]]` sets a HOME-relative `path` and either literal `content` or a `package` and `source` pair. The pair selects a relative path inside a package output. Both paths must have nonempty components other than `.` or `..` and must be free of NUL bytes. Home file paths must stay clear of `.local/state/vug`, `.local/share/vug/home`, and `.config/systemd/user`, including their parents and descendants.
- Each `[[unit]]` sets a systemd unit `name` and its public `content` as a NUL-free string. Optional `enable` and `secrets` list target unit names and logical secret names, respectively. The example preserves the `{pkg:hello}` reference in the unit text.
- Each `[[secret]]` declares a logical `name`. The record carries the name alone.

Composition takes the union of the home and its selected fragments. Packages, files, variables, units, and secrets are keyed by package name, destination path, variable name, unit name, and secret name, respectively. Identical definitions collapse to a single entry. Different definitions for one key and file paths nested inside other declared file paths are errors, including across records. Package-backed files must refer to selected packages, and units must refer to declared secrets. These references can be satisfied by any record in the union. The schema rejects unknown keys in each record and in its entries.

## Commands

- `vug home format-home --file PATH [--write]` validates and formats one home record. It retains its fragment selections.
- `vug home format-fragment --file PATH [--write]` validates and formats one fragment.
- `vug home compose-home --root DIR --name NAME` reads the named home and its fragments and prints a single canonical TOML record containing their combined entries.

Formatting sorts names and paths, removes repeated set members and identical definitions, and omits empty collections. With `--output text` each command prints TOML. The default JSON output includes the TOML string under `Record`. Formatting also reports `Changed` and rewrites the input only with `--write`. Input errors exit with status 254.

## Building generations

```console
$ vug home build-home --root . --name desktop --binary-cache https://cache.example --trusted-key PUBLIC
$ vug home delete-generations --keep 3
$ vug home collect-garbage --dry-run
$ vug home collect-garbage
```

`build-home` composes the record, resolves the greatest version of each selected package, and publishes a content-addressed output and a numbered generation. Resolution uses the user store's existing index and the [binary cache's signed keys and verified trees](cache.md#use). An unavailable package fails the command before a generation is recorded. The manifest records declared runtime dependencies and store references found in package files and symlink targets, including references to dynamic loaders.

| Output | Contents |
|---|---|
| `sw/` | Relative links to selected package outputs, with shared directories merged. Two different outputs providing the same leaf path cause an error. |
| `files/` | Literal content or copies of regular files within the selected package outputs, under their HOME-relative destinations. |
| `units/` | Unit files and relative `<target>.wants/` links. Unit text expands `{pkg:NAME}` to the package's canonical `/vug/store/<id>` path. |
| `environment/` | Literal variables in `variables.json` and the profile-relative PATH prefix `sw/bin` in `path`. |
| `manifest/home.json` | Home name, package names mapped to store IDs, the transitive closure of store IDs, file destinations, unit targets, and logical secret names. |

The output ID covers the public output tree, including the manifest. The build publishes its own directories and files with read-only permissions. Relative package links resolve in the local user store. Packages with embedded `/vug/store` references need the user store to be mounted at that path for execution, as with `vug package run-package`. Building a generation leaves activation, login integration, and secret preparation to the commands that consume it.

The home commands share a dedicated user data directory (default `${XDG_DATA_HOME:-$HOME/.local/share}/vug/home`) and state directory (default `${XDG_STATE_HOME:-$HOME/.local/state}/vug/home`). Use `--data-dir` and `--state-dir` to select a dedicated pair. Both directories must be owned by the caller and writable only by their owner. The state directory's `store` link points to the actual user store. The numbered links under `generations/` point into that same store. A build that produces the newest generation's output again returns the same number. The build's default JSON output gives its `Name`, `Id`, `Path` in the user store, and `Generation`.

`delete-generations` keeps the newest `--keep` links (at least one) and the generation named by the state's `current` link, when present. `collect-garbage` retains every remaining generation and its closure. Both commands use the same locks as `build-home`. Run cleanup against the same state and data directories used for the build.

## Activating a generation

```console
$ vug home switch-home --generation 2
$ vug home rollback-home
```

`switch-home` selects a generation that `build-home` has created. `rollback-home` selects the nearest earlier retained generation. Both take the same `--state-dir` and `--data-dir` options as `build-home` and report the activated `Generation`. Run them as the owner of HOME. The profile records the resolved path of that HOME directory and the last completed generation. Subsequent switches check both records against HOME and `current`.

Files, unit files, and target wants links are installed under HOME as symlinks through the profile's `current` link. Both commands check existing and prospective destinations before changing any home links. A hand-placed file or symlink at a new destination, a changed or missing managed link, or a symlink in a destination's parent path causes an error. Move conflicting files aside or restore the expected managed link before retrying. Activation preserves existing directories and application data. It creates directories as needed and leaves those directories in place when their managed links are removed. Login integration, systemd user-manager operations, and secret preparation are separate from switching generations.

Activation records a pending switch under the profile lock before changing links, then replaces `current` by renaming a symlink into place. Destinations common to both generations follow `current`. If activation fails or is interrupted before the completed generation is recorded, recovery restores the prior set of links and `current`. Each activation command checks for and recovers an unfinished switch before applying its request. If recovery encounters a path that was changed by hand, it preserves that path and retains the pending record for a later retry. Generation deletion retains both generations in a pending switch, and user-store garbage collection runs under the same lock.
