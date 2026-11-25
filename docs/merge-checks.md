# Merge checks

`vug record decide-merge` classifies a candidate Git diff, runs the scope check, and computes a decision from the required check results. The command accepts a JSON value, a `file://` URI, or `file:///dev/stdin`.

```console
vug record decide-merge --input file://request.json
```

## Request format

The input is an object with these fields.

| Field | Value |
| --- | --- |
| `schema_version` | `1` |
| `identity` | An object with `base_sha`, `head_sha`, `tree_oid` and `policy_digest`. The Git IDs use the same object format (40 or 64 lowercase hex digits). The policy digest is 64 lowercase hex digits. |
| `core_packages` | Package names from the policy identified by `policy_digest`. |
| `changes` | Changed entries from the base tree to the candidate tree, with one entry per path. |
| `required_checks` | Objects with a numeric `check` and a nonempty string `target`. |
| `results` | Objects with `check`, `target`, `identity`, `status`, and an optional `output_id`. |

Each changed entry has a repository-relative `path` and two sides, `before` and `after`. Use a null side for an absent entry. Split a rename into a deletion and an addition. Each present side has a Git `mode` string and, for a regular package or version record, its UTF-8 TOML `text`. For other entries, callers may omit `text`. Both sides are read as Git objects by the caller.

Check 2 is computed from the diff. For each package touched, Checks 1 (format), 3 (source origins), 4 (source hashes), 5 (patch provenance), 6 (build and tests), 7 (reverse dependencies), and 8 (reproducibility) are required, with the package name as the target. Check 1 also requires the target `.` when the diff affects a path outside `packages/` or has no recognized package. The policy can enumerate additional targets. A check result must have the same identity as the request and match one required check and target. The supported statuses are `passed`, `failed` and `incomplete`. Checks 3 and 5 may also report `requires_approval` for completed checks whose provenance needs a policy exception.

The caller collects and supplies the complete Git diff, authenticates the policy and results against the candidate identity, runs the required checks, verifies approvals, and applies the decision. To produce results for Checks 3–5, use `vug record verify-provenance`.

## Scope

- `record_change` covers changes confined to recognized package, version and patch paths with regular nonexecutable files and no approval condition.
- `approval_change` covers changes inside `packages/` that require review. This includes changing an existing `[source]` or `[upstream]` table, deleting a version, touching a core package, adding an arbitrary build command or a nonstandard path, using an executable file, and weakening test settings of an existing version. For a newly added version the policy still requires Check 6, including its mandatory tests.
- `repository_change` takes precedence if any path is outside `packages/`.

The scope check compares parsed TOML values. An omitted source `strip-components` means `1`. Modes other than `100644` and `100755`, duplicate paths, and empty diffs fail the scope check. When Check 3 or 5 requires approval, a package-only diff is reported as `approval_change` and the independently computed `Scope` still shows the diff check's own result.

## Decision and output

The JSON report echoes the candidate identity and schema version. It includes `ChangeClass`, `Scope` (`passed`, `requires_approval`, or `failed`), `Decision`, `AffectedPackages`, `Reasons`, and `IncompleteChecks`. Reasons use stable codes with a path or check target where applicable.

| Decision | Condition |
| --- | --- |
| `reject` | The scope or a required check failed, or results have duplicate or unexpected targets, invalid statuses for their check, or mismatched identities. A duplicated required target also rejects the request. |
| `incomplete` | In the absence of a rejection, at least one required target or result is missing or unfinished. |
| `requires_approval` | All required checks finished without a rejection and the diff or provenance requires approval. |
| `auto_merge` | The diff is a record change and all required checks passed. |

Exit status 0 means the command computed a report. Read `Decision` to determine the outcome. An unreadable input or invalid request exits 254. Text and table output show the decision alone.

## Source and patch provenance

```console
vug record verify-provenance --root /path/to/repository --input file://candidate.json --policy file://policy.json --source-cache-dir verified-sources
```

The command reads Git objects under `--root`. The input contains `schema_version: 1` and the same `identity` object as `decide-merge`. The caller authenticates the base and head commits, the candidate tree and the policy. The command checks their Git object types and verifies that the lowercase SHA-256 of the policy JSON bytes equals `identity.policy_digest`.

The policy is a JSON object with these fields.

| Field | Value |
| --- | --- |
| `schema_version` | `1` |
| `nixpkgs_commit` | The full lowercase 40-digit commit ID for the nixpkgs sources and patches used by this policy. |
| `new_package_sources` | Map of package names to source URL prefixes read from recipes at that nixpkgs commit. |
| `approved_sources` | Map of package names to additional source URL prefixes approved by a maintainer. |
| `redirect_sources` | Map of package names to extra destination URL prefixes for redirects of previously approved source URLs. |
| `patch_origins` | Map of candidate patch paths to an approved pinned file. A nixpkgs pin has `kind: "nixpkgs"` and a repository-relative `path`. An upstream pin has `kind: "upstream"`, the HTTPS `repository` prefix, a `raw_root` HTTPS prefix, a full lowercase 40-digit `commit`, and a repository-relative `path`. The maintainer maps `raw_root` to the repository's endpoint for immutable files. The verifier appends `<commit>/<path>` to that prefix. |

Use an empty object for a map with no entries. The policy publisher checks the fixed nixpkgs recipes and approved upstream repositories when assembling the policy.

- **Check 3** verifies each affected version's source against both the candidate's declared source prefixes and the trusted set. The trusted set comes from the base package record or the fixed nixpkgs mapping for a new package, plus any explicitly approved source prefixes. HTTPS origin prefixes end in a slash. Matching compares the parsed host, effective port, and path prefix. A URL outside the candidate declarations fails. A URL declared only by the candidate requires approval.
- **Check 4** fetches the approved source over HTTPS, computes SHA-256 from the response bytes, and compares it to the version record. Each redirect must match a trusted source prefix or an explicitly allowed redirect prefix. Downloads use public IP addresses, at most five redirects, a 30-second timeout per request including the body, and a 512 MiB limit per source. A hash mismatch or a redirect outside the approved prefixes fails. An unavailable download leaves the check incomplete.
- **Check 5** compares candidate patch bytes with the file at a fixed nixpkgs or approved upstream commit. The associated upstream repository must cover a trusted source prefix. A patch outside the pinned origins requires approval. Different bytes at a pinned origin fail. An unavailable download leaves the check incomplete. Patch downloads use the same network restrictions, a fixed destination and an 8 MiB limit.

The package diff determines the affected records. A changed version selects its referenced patches. A changed patch selects versions that reference it. A changed package record selects all its candidate versions. Check 4 runs after Check 3 passes for the package. Check 5 runs after Check 4 passes. A package without applicable inputs for a check passes that check when its prerequisites pass.

Copy the JSON report's `Results` array into `decide-merge`'s `results` alongside the other required checks. Each row carries the candidate identity, package target, check number and status. `Findings` gives the affected path and reason code. Exit status 0 means the command produced a report. Invalid input, policy data or object data exits 254.

The provenance gate verifies public HTTPS sources. A Git over SSH source in a version under review produces a `private_git_source` finding and requires a local build with the user's SSH credentials.

With `--source-cache-dir`, the command writes each matched source archive and patch to the named directory as a file named by its lowercase SHA-256. The files are published individually after verification. Use the directory for a build only after checking that the required provenance results passed and supplying verified seed and dependency inputs. Pass the directory to `vug package build-package --source-cache-dir`.
