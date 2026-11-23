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

The caller supplies the complete diff and authenticates the policy and check results against the candidate's identity. Git object collection, verification of check authenticity, checks 1 and 3–8, approval verification, and applying the decision are responsibilities of the caller.

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
