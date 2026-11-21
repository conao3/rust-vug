# vug

`vug` is a declarative package manager and system builder. The repository
currently ships the project scaffolding and one command,
`vug version compare-versions`, which compares two version strings.

## Lean and Rust

- The deterministic core — version parsing, version ordering, and the process
  exit statuses — is defined in Lean inside the `leanfran::lean!` block in
  `src/version.rs`, in the `model` namespace, and generated to Rust by
  `leanfran-build`.
- Rust owns the CLI (`clap`), `file://` input resolution, JSON/text/table
  output, and process exit codes. It does not reimplement the ordering.
- Lean definitions:
  - `Component` (`num`/`text`), `RunKind` — parsed component types.
  - `isSeparator`, `digitsToNat`, `runComponent`, `flush`, `parseLoop`,
    `parse` — the version parser.
  - `cmpNat`, `flipOrd`, `compareChars`, `compareList`, `compare` — the
    ordering.
  - `usageStatus` (252), `failureStatus` (254), `otherStatus` (255) — process
    exit statuses.
- Lean theorems:
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

## Generation and verification

- `cargo build` inside `nix develop` (which sets `LEANFRAN_CLI`) regenerates
  `lean/` — the embedded Lean source as a lake package — and
  `.leanfran/cache/` — the generated Rust. Both are committed; `make check`
  verifies they are fresh.
- `audit/` is a separate lake package that audits `model.compare` with
  `#assay` (leanassay): every mutant must be killed by at least one collected
  theorem. The exit-status constants are outside the audit: a theorem about a
  constant would only repeat its value.
- `nix develop --command make check` runs `nix fmt -- --ci`,
  `cargo clippy --all-targets -- -D warnings`, `lake -d lean build --wfail`,
  `lake -d audit build --wfail`, the generated-file freshness check, and
  `cargo test`.
- Exit codes: 0 success, 252 usage errors, 254 errors in the input being
  processed (for example an unreadable `file://` path), 255 anything else.
  `help` works at every command level (`vug help`, `vug version help`,
  `vug version compare-versions help`).
