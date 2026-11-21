# vug

`vug` is a declarative package manager and system builder. The repository
ships `vug version compare-versions`, which compares two version strings, and
the `vug record` commands of `docs/record.md`, which validate package records.

## Lean and Rust

- The deterministic core — version parsing, version ordering, name and version
  syntax, record paths, and the process exit statuses — is defined in Lean
  inside `leanfran::lean!` blocks in the `model` namespace and generated to
  Rust by `leanfran-build`. `src/version.rs` holds the ordering;
  `src/naming.rs` holds the record-name and path rules.
- Rust owns the CLI (`clap`), `file://` input resolution, TOML parsing, the
  canonical record writer, JSON/text/table output, and process exit codes. It
  does not reimplement the ordering or the name and path rules.
- Lean definitions (`version`):
  - `Component` (`num`/`text`), `RunKind` — parsed component types.
  - `isSeparator`, `digitsToNat`, `runComponent`, `flush`, `parseLoop`,
    `parse` — the version parser.
  - `cmpNat`, `flipOrd`, `compareChars`, `compareList`, `compare` — the
    ordering.
  - `usageStatus` (252), `failureStatus` (254), `otherStatus` (255) — process
    exit statuses.
- Lean definitions (`naming`):
  - `isNameFirstChar`, `isNameChar`, `isVersionFirstChar`, `isVersionChar`,
    `nameCharsOk`, `versionCharsOk` — character classes.
  - `tomlSuffix`, `endsWithToml` — the `.toml` suffix test.
  - `validName`, `validVersion` — the "Names and versions" rules of
    `docs/record.md`.
  - `shard`, `packageDir`, `versionPath` — the record paths.
- Lean theorems (`version`):
  - `compare_refl` — `compare a a = .eq` for every string.
  - `compare_flip` — `compare b a = flipOrd (compare a b)`.
  - `compare_eq_iff_parse_eq` — `compare a b = .eq ↔ parse a = parse b`.
  - `compare_lt_swap`, `compare_gt_swap`, `compare_eq_swap` — argument
    reversal, derived from `compare_flip`.
  - `compare_lt_trans` — transitivity of the strict order, built on
    `compareList_lt_trans`.
  - Helper lemmas: `cmpNat_eq_iff`, `cmpNat_lt_iff`, `cmpNat_gt_iff`,
    `cmpNat_flip`, `compareChars_eq_iff`, `compareChars_flip`,
    `compareChars_lt_trans`, `compareList_eq_iff`, `compareList_flip`,
    `compareList_lt_trans`.
- Lean theorems (`naming`):
  - `validName_no_slash`, `validVersion_no_slash` — a valid name or version
    contains no `/`.
  - `validName_no_upper` — a valid name contains no uppercase ASCII letter.
  - `validName_nonempty`, `validVersion_nonempty` — a valid name or version
    is not empty.
  - `packageDir_inj` — `packageDir` is injective on valid names.
  - `versionPath_inj` — `versionPath` is injective on valid names and
    versions.
  - `packageDir_no_prefix` — for valid names `a ≠ b`, `packageDir a ++ "/"`
    is not a prefix of `versionPath b v`.
  - Helper lemmas: character-class membership and range facts
    (`isNameChar_toNat`, `isVersionChar_toNat`, `isNameChar_ne_slash`,
    `isVersionChar_ne_slash`, …), `shard_toList`, `shard_length`,
    `packageDir_toList`, `versionPath_toList`, `packageDir_suffix`,
    `prefix_slash_mem`, `suffix_append_cases`, `prefixLen`, and `decide`d
    ground-truth facts (`validName_hello`, `shard_single`, …) for the audit.
- Record commands (`src/record.rs`): `record format-record --file PATH
  [--write]`, `record check-record --file PATH`, `record check-tree --root
  DIR`. TOML is parsed with the `toml` crate; the canonical writer is written
  by hand to the rules of `docs/record.md`.

## Generation and verification

- `cargo build` inside `nix develop` (which sets `LEANFRAN_CLI`) regenerates
  `lean/` — the embedded Lean source as a lake package — and
  `.leanfran/cache/` — the generated Rust. Both are committed; `make check`
  verifies they are fresh.
- `audit/` is a separate lake package that audits the deterministic functions
  with `#assay` (leanassay): every mutant must be killed by at least one
  collected theorem. `Audit/Version.lean` covers `model.compare`;
  `Audit/Naming.lean` covers `model.validName`, `model.validVersion`,
  `model.shard`, `model.packageDir`, and `model.versionPath`. The exit-status
  constants are outside the audit: a theorem about a constant would only
  repeat its value.
- `nix develop --command make check` runs `nix fmt -- --ci`,
  `cargo clippy --all-targets -- -D warnings`, `lake -d lean build --wfail`,
  `lake -d audit build --wfail`, the generated-file freshness check, and
  `cargo test`.
- Exit codes: 0 success, 252 usage errors, 254 errors in the input being
  processed (for example an unreadable `file://` path or an invalid record),
  255 anything else. `help` works at every command level (`vug help`,
  `vug record help`, `vug record check-tree help`).
