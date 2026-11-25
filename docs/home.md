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
- Each `[[file]]` sets a HOME-relative `path` and either literal `content` or a `package` and `source` pair. The pair selects a relative path inside a package output. Both paths must have nonempty components other than `.` or `..` and must be free of NUL bytes. Home file paths must stay clear of `.local/state/vug`, `.local/share/vug/home`, `.config/systemd/user`, and `.config/vug/home`, including their parents and descendants.
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

`build-home` composes the record, resolves the greatest version of each selected package, and publishes a content-addressed output and a numbered generation. Resolution uses the user store's existing index and the [binary cache's signed keys and verified trees](cache.md#use). Nix outputs are verified and acquired directly from the [Nix binary cache](build.md#nix-outputs). An unavailable package fails the command before a generation is recorded. The manifest records declared runtime dependencies and store references found in package files and symlink targets, including references to dynamic loaders.

For [Nix output records](record.md#build-kind-nix-output), `build-home` acquires the signed Nix closure directly from `cache.nixos.org` and keeps it in the home data directory under `nix/store`.

| Output | Contents |
|---|---|
| `sw/` | Relative links to selected package outputs, with shared directories merged. Two different outputs providing the same leaf path cause an error. |
| `files/` | Literal content or copies of regular files within the selected package outputs, under their HOME-relative destinations. |
| `units/` | Unit files and relative `<target>.wants/` links. Unit text expands `{pkg:NAME}` to the package's canonical `/vug/store/<id>` path. |
| `commands/` | Launchers for the programs exported by selected packages in `sw/bin`. |
| `environment/` | Literal variables, a shell profile, and a systemd environment file. |
| `manifest/home.json` | Home name, package names mapped to store IDs, the transitive closure of store IDs, file destinations, unit targets, and logical secret names. |
| `manifest/nix-closure.json` | When the generation uses Nix outputs, the deduplicated Nix store paths, signed NAR hashes and sizes, and references. |

The output ID covers the public output tree, including the manifest. The build publishes its own directories and files with read-only permissions. Relative package links resolve in the local user store. The launchers mount the home package closure at its canonical paths for the duration of the command. Build a new generation to generate the launchers and user service environment files.

The home commands share a dedicated user data directory (default `${XDG_DATA_HOME:-$HOME/.local/share}/vug/home`) and state directory (default `${XDG_STATE_HOME:-$HOME/.local/state}/vug/home`). Use `--data-dir` and `--state-dir` to select a dedicated pair. Both directories must be owned by the caller and writable only by their owner. The state directory's `store` link points to the actual user store. The numbered links under `generations/` point into that same store. A build that produces the newest generation's output again returns the same number. The build's default JSON output gives its `Name`, `Id`, `Path` in the user store, and `Generation`.

`delete-generations` keeps the newest `--keep` links (at least one) and the generation named by the state's `current` link, when present. `collect-garbage` retains each remaining generation, its closure, and the closures of running launchers. It reclaims unreferenced vug-owned Nix paths in the user data directory along with unreferenced vug outputs. Generation operations share the profile lock with `build-home`. Garbage collection waits for builds and launcher startup. Run cleanup against the same state and data directories used for the build.

## Activating a generation

```console
$ vug home switch-home --generation 2
$ vug home rollback-home
```

`switch-home` selects a generation that `build-home` has created. `rollback-home` selects the nearest earlier retained generation. Both take the same `--state-dir` and `--data-dir` options as `build-home` and report the activated `Generation`. Run them as the owner of HOME. The profile records the resolved path of that HOME directory and the last completed generation. Subsequent switches check both records against HOME and `current`.

Files, unit files, and target wants links are installed under HOME as symlinks through the profile's `current` link. Both commands check existing and prospective destinations before changing any home links. A hand-placed file or symlink at a new destination, a changed or missing managed link, or a symlink in a destination's parent path causes an error. Move conflicting files aside or restore the expected managed link before retrying. Activation preserves existing directories and application data. It creates directories as needed and leaves those directories in place when their managed links are removed.

Activation records a pending switch under the profile lock before changing links, then replaces `current` by renaming a symlink into place. Destinations common to both generations follow `current`. If activation fails or is interrupted before the completed generation is recorded, recovery restores the prior set of links and `current`. Each activation command checks for and recovers an unfinished switch before applying its request. If recovery encounters a path that was changed by hand, it preserves that path and retains the pending record for a later retry. Generation deletion retains both generations in a pending switch, and user-store garbage collection runs under the same lock.

## Login and user services

With the `home-login` service included in a Vug system, source its profile fragment from `/etc/profile`:

```sh
. /etc/profile.d/vug-home.sh
```

The fragment runs in a non-root login shell. It loads the active generation's public variables and prepends its `commands/` directory to the login PATH. It uses the public profile link under `~/.config/vug/home/`. Start a fresh login shell after switching to load the new variables.

The launcher uses `vug home run-program` and requires bubblewrap with unprivileged user namespaces enabled. Each launch keeps the system's store visible and mounts the selected user closure at `/vug/store/<id>` for that process. When an ID appears in both stores, the launcher verifies the content and uses the system's copy. The system store and the user store are mounted read-only inside the command. When the user closure includes Nix outputs, the launcher also mounts its selected paths at `/nix/store` within the command’s namespace.

When that generation includes a Nix closure, the launcher also mounts a private read-only `/nix/store` containing only the Nix paths declared by the selected generation's package outputs and their runtime dependencies. The original absolute store paths resolve inside the launcher's mount namespace.

An `ExecStart` whose executable starts with `{pkg:NAME}/` launches that file from the user generation with the same mechanism. For a command provided by the system, use its absolute path. Managed services pick up the new generation's public variables from an `EnvironmentFile`. Use an enable target appropriate to each service, timer, or socket. For a graphical session service, select the graphical session target that your desktop activates.

With a running user manager, switching stops removed active units, reloads unit definitions, restarts changed active units, and starts newly enabled units whose targets are active. The prior unit configuration is restored during recovery from an interrupted switch. When there is no user manager, the command installs the files and reports `UnitsPending`. The user manager reads the unit links at its next start. Login uses the standard HOME-relative systemd user-unit directory.

## User service credentials

Declare logical secret names with `[[secret]]` and list the names each service consumes in its `secrets` array. The generation generates a systemd dependency on `vug-home-secrets.service` and sets `LoadCredential=` for each name. In a service, open the file with that name inside `$CREDENTIALS_DIRECTORY`. Credentials are accessible within the trust boundary of the current UID and system administrators.

Provide SOPS at `/bin/sops` and encrypt the source documents with age. Keep them outside the store. Create `~/.config/vug/home/secrets/` with mode `0700`, owned by the login user. Place the age identity in `age.key` and the source map in `sources.toml` in that directory. Both files must be regular files owned by the login user with mode `0600`.

```toml
[example-key]
document = "/home/example/private/credentials.yaml"
field = ["services", "example"]
```

Use an absolute document path and a list of keys from the root of the document to a string value. Keys can contain ASCII letters, digits, underscores, and hyphens. SOPS writes its extracted value directly to a mode-`0600` file in a private directory under `$XDG_RUNTIME_DIR/vug/`. The user manager receives the file path. A failed SOPS invocation reports the logical name so its diagnostics cannot disclose the decrypted value.

A switch with a running user manager decrypts all declared values before replacing the active generation. If a source is missing or fails to decrypt, the current generation and its running services keep their credentials. Setting the same generation again refreshes the credentials and restarts running services that use them. A rollback resolves the selected generation's logical names against the current source map and encrypted documents.

At login, systemd runs the preparation service before starting services that require credentials. A failure prevents those services from starting. The private runtime snapshot lives for the current runtime directory's lifetime, including when systemd user lingering keeps the directory alive. Successful activation removes the prior snapshot. Rebuild home outputs built by older releases to add the generated credential service.
