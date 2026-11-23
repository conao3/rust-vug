leanfran::lean! {
    pub mod model;
    r#"
inductive ChangeClass where
  | recordChange
  | approvalChange
  | repositoryChange
  deriving BEq, ReflBEq, LawfulBEq, DecidableEq

structure Change where
  path : String
  sourceChanged : Bool
  versionDeleted : Bool
  policyApproval : Bool

def inPackages (path : String) : Bool :=
  "packages/".toList.isPrefixOf path.toList

def approvalNeeded (change : Change) : Bool :=
  change.sourceChanged || change.versionDeleted || change.policyApproval

def classify (changes : List Change) : ChangeClass :=
  if changes.any (fun c => !inPackages c.path) then .repositoryChange
  else if changes.any approvalNeeded then .approvalChange
  else .recordChange

theorem classify_record_iff (changes : List Change) :
    classify changes = .recordChange ↔
      ∀ c ∈ changes, inPackages c.path = true ∧
        c.sourceChanged = false ∧ c.versionDeleted = false ∧ c.policyApproval = false := by
  unfold classify
  split <;> rename_i outside
  · simp [List.any_eq_true] at outside
    obtain ⟨c, hm, h⟩ := outside
    simp only [reduceCtorEq, false_iff]
    intro hAll
    exact Bool.noConfusion (h.symm.trans (hAll c hm).1)
  · split <;> rename_i approval
    · simp [List.any_eq_true] at approval
      obtain ⟨c, hm, h⟩ := approval
      simp only [reduceCtorEq, false_iff]
      intro hAll
      obtain ⟨_, hs, hd, hp⟩ := hAll c hm
      simp [approvalNeeded, hs, hd, hp] at h
    · simp [List.any_eq_true, approvalNeeded, Bool.or_eq_true] at outside approval
      constructor
      · intro _ c hm
        obtain ⟨⟨hs, hd⟩, hp⟩ := approval c hm
        exact ⟨outside c hm, hs, hd, hp⟩
      · intro _
        rfl

theorem classify_nonrecord (changes : List Change) :
    classify changes = .repositoryChange ↔ ∃ c ∈ changes, inPackages c.path = false := by
  unfold classify
  by_cases outside : changes.any (fun c => !inPackages c.path) = true
  · have found := List.any_eq_true.mp outside
    simpa [outside] using found
  · simp [outside]
    split <;> (simp; simpa [List.any_eq_true] using outside)

theorem classify_append_swap (a b : List Change) :
    classify (a ++ b) = classify (b ++ a) := by
  simp [classify, List.any_append, Bool.or_comm]

theorem classify_packages_path (suffix : String) :
    classify [⟨"packages/" ++ suffix, false, false, false⟩] = .recordChange := by
  simp [classify, inPackages, approvalNeeded]

theorem classify_plain_repository :
    classify [⟨"README.md", false, false, false⟩] = .repositoryChange := by decide

inductive CheckStatus where
  | passed
  | failed
  | incomplete
  | requiresApproval
  deriving BEq, ReflBEq, LawfulBEq, DecidableEq

inductive Decision where
  | autoMerge
  | requiresApproval
  | reject
  | incomplete
  deriving BEq, ReflBEq, LawfulBEq, DecidableEq

def decideMerge (kind : ChangeClass) (checks : List CheckStatus) : Decision :=
  if checks.contains .failed then .reject
  else if checks.contains .incomplete then .incomplete
  else if kind = .recordChange ∧ !checks.contains .requiresApproval = true then .autoMerge
  else .requiresApproval

theorem decideMerge_auto_iff (kind : ChangeClass) (checks : List CheckStatus) :
    decideMerge kind checks = .autoMerge ↔
      kind = .recordChange ∧ ∀ check ∈ checks, check = .passed := by
  have allPassed : (.failed ∉ checks ∧ .incomplete ∉ checks ∧ .requiresApproval ∉ checks) ↔
      ∀ check ∈ checks, check = .passed := by
    constructor
    · intro ⟨hf, hi, ha⟩ check hm
      cases check
      · rfl
      · exact False.elim (hf hm)
      · exact False.elim (hi hm)
      · exact False.elim (ha hm)
    · intro h
      refine ⟨?_, ?_, ?_⟩
      · intro hm
        cases h .failed hm
      · intro hm
        cases h .incomplete hm
      · intro hm
        cases h .requiresApproval hm
  rw [← allPassed]
  by_cases hf : .failed ∈ checks <;> by_cases hi : .incomplete ∈ checks <;>
    cases kind <;> simp [decideMerge, List.contains_eq_mem, hf, hi]

theorem decideMerge_reject_iff (kind : ChangeClass) (checks : List CheckStatus) :
    decideMerge kind checks = .reject ↔ .failed ∈ checks := by
  by_cases hf : .failed ∈ checks <;> by_cases hi : .incomplete ∈ checks <;>
    cases kind <;> simp [decideMerge, List.contains_eq_mem, hf, hi] <;> split <;> simp_all

theorem decideMerge_incomplete_iff (kind : ChangeClass) (checks : List CheckStatus) :
    decideMerge kind checks = .incomplete ↔ .failed ∉ checks ∧ .incomplete ∈ checks := by
  by_cases hf : .failed ∈ checks <;> by_cases hi : .incomplete ∈ checks <;>
    cases kind <;> simp [decideMerge, List.contains_eq_mem, hf, hi] <;> split <;> simp_all

theorem decideMerge_append_swap (kind : ChangeClass) (a b : List CheckStatus) :
    decideMerge kind (a ++ b) = decideMerge kind (b ++ a) := by
  simp [decideMerge, Bool.or_comm]

def originMatches (originHost : String) (originPort : Nat) (originPath : String)
    (host : String) (port : Nat) (path : String) : Bool :=
  originHost == host && originPort == port && originPath.toList.isPrefixOf path.toList

theorem originMatches_iff (oh h op p : String) (on n : Nat) :
    originMatches oh on op h n p = true ↔ oh = h ∧ on = n ∧ op.toList <+: p.toList := by
  simp [originMatches, List.isPrefixOf_iff_prefix, and_assoc]

theorem originMatches_append (h op suffix : String) (port : Nat) :
    originMatches h port op h port (op ++ suffix) = true := by
  rw [originMatches_iff]
  exact ⟨rfl, rfl, by simp⟩

"#
}

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::naming;

const PACKAGE_CHECKS: [u8; 7] = [1, 3, 4, 5, 6, 7, 8];

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub(crate) base_sha: String,
    pub(crate) head_sha: String,
    pub(crate) tree_oid: String,
    pub(crate) policy_digest: String,
}

impl Identity {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let oid_len = self.base_sha.len();
        if !matches!(oid_len, 40 | 64)
            || ![&self.base_sha, &self.head_sha, &self.tree_oid]
                .into_iter()
                .all(|oid| is_hex(oid, oid_len))
            || !is_hex(&self.policy_digest, 64)
        {
            return Err(
                "expected Git object IDs and a 64-digit policy digest in lowercase hex".into(),
            );
        }
        Ok(())
    }
}

pub(crate) fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    identity: Identity,
    core_packages: BTreeSet<String>,
    changes: Vec<DiffEntry>,
    required_checks: Vec<Target>,
    results: Vec<CheckResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffEntry {
    path: String,
    before: Option<Blob>,
    after: Option<Blob>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    mode: String,
    text: Option<String>,
}

impl DiffEntry {
    fn blobs(&self) -> impl Iterator<Item = &Blob> {
        [&self.before, &self.after].into_iter().flatten()
    }

    fn has_unsupported_mode(&self) -> bool {
        self.blobs()
            .any(|blob| !matches!(blob.mode.as_str(), "100644" | "100755"))
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct Target {
    #[serde(rename(serialize = "Check"))]
    check: u8,
    #[serde(rename(serialize = "Target"))]
    target: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckResult {
    pub(crate) identity: Identity,
    pub(crate) check: u8,
    pub(crate) target: String,
    pub(crate) status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Passed,
    Failed,
    Incomplete,
    RequiresApproval,
}

impl Status {
    pub(crate) fn combine(self, other: Self) -> Self {
        match model::decide_merge(
            model::ChangeClass::RecordChange,
            vec![self.into(), other.into()],
        ) {
            model::Decision::AutoMerge => Self::Passed,
            model::Decision::Reject => Self::Failed,
            model::Decision::Incomplete => Self::Incomplete,
            model::Decision::RequiresApproval => Self::RequiresApproval,
        }
    }
}

impl From<Status> for model::CheckStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::Passed => Self::Passed,
            Status::Failed => Self::Failed,
            Status::Incomplete => Self::Incomplete,
            Status::RequiresApproval => Self::RequiresApproval,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Report {
    schema_version: u32,
    base_sha: String,
    head_sha: String,
    tree_oid: String,
    policy_digest: String,
    change_class: &'static str,
    scope: &'static str,
    pub(crate) decision: &'static str,
    affected_packages: BTreeSet<String>,
    reasons: Vec<Reason>,
    incomplete_checks: BTreeSet<Target>,
}

#[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "PascalCase")]
struct Reason {
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

impl Reason {
    fn path(code: &'static str, path: &str) -> Self {
        Self {
            code,
            path: Some(path.into()),
            check: None,
            target: None,
        }
    }

    fn check(code: &'static str, target: &Target) -> Self {
        Self {
            code,
            path: None,
            check: Some(target.check),
            target: Some(target.target.clone()),
        }
    }
}

pub(crate) fn evaluate(json: &str) -> Result<Report, String> {
    let request: Request = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if request.schema_version != 1 {
        return Err("expected schema_version 1".into());
    }
    request.identity.validate()?;
    if request
        .core_packages
        .iter()
        .any(|p| !naming::model::valid_name(p))
    {
        return Err("core_packages contains an invalid package name".into());
    }

    let mut paths = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let mut changes = Vec::new();
    let mut reasons = Vec::new();
    let mut checks = Vec::new();
    let mut scope_rejected = request.changes.is_empty();
    if scope_rejected {
        reasons.push(Reason {
            code: "empty_diff",
            path: None,
            check: None,
            target: None,
        });
    }
    for entry in &request.changes {
        if entry.path.contains('\0')
            || entry.path.split('/').any(|p| matches!(p, "" | "." | ".."))
            || (entry.before.is_none() && entry.after.is_none())
        {
            return Err(format!("invalid diff entry for {:?}", entry.path));
        }
        if !paths.insert(&entry.path) {
            scope_rejected = true;
            reasons.push(Reason::path("duplicate_path", &entry.path));
        }
        if entry.has_unsupported_mode() {
            scope_rejected = true;
            reasons.push(Reason::path("unsupported_git_mode", &entry.path));
        }
        changes.push(inspect(
            entry,
            &request.core_packages,
            &mut packages,
            &mut reasons,
        )?);
    }
    let kind = model::classify(changes);
    let repository_change = matches!(kind, model::ChangeClass::RepositoryChange);
    let scope = if scope_rejected {
        checks.push(model::CheckStatus::Failed);
        "failed"
    } else if matches!(kind, model::ChangeClass::RecordChange) {
        "passed"
    } else {
        "requires_approval"
    };

    let mut targets = BTreeMap::new();
    for target in request.required_checks {
        if !PACKAGE_CHECKS.contains(&target.check) || target.target.trim().is_empty() {
            return Err("invalid required check target".into());
        }
        if targets.insert(target.clone(), None).is_some() {
            reasons.push(Reason::check("duplicate_required_target", &target));
            checks.push(model::CheckStatus::Failed);
        }
    }

    let mut incomplete_checks = BTreeSet::new();
    let mandatory = packages.iter().flat_map(|package| {
        PACKAGE_CHECKS.into_iter().map(move |check| Target {
            check,
            target: package.clone(),
        })
    });
    let repository_format = (repository_change || packages.is_empty()).then(|| Target {
        check: 1,
        target: ".".into(),
    });
    for target in mandatory.chain(repository_format) {
        if !targets.contains_key(&target) {
            reasons.push(Reason::check("missing_required_target", &target));
            checks.push(model::CheckStatus::Incomplete);
            incomplete_checks.insert(target);
        }
    }

    let mut grouped = BTreeMap::<_, Vec<_>>::new();
    for result in request.results {
        let target = Target {
            check: result.check,
            target: result.target.clone(),
        };
        grouped.entry(target).or_default().push(result);
    }
    for (target, group) in grouped {
        let [result] = group.as_slice() else {
            reasons.push(Reason::check("duplicate_result", &target));
            checks.push(model::CheckStatus::Failed);
            continue;
        };
        let Some(status) = targets.get_mut(&target) else {
            reasons.push(Reason::check("unexpected_result", &target));
            checks.push(model::CheckStatus::Failed);
            continue;
        };
        if result.identity != request.identity {
            reasons.push(Reason::check("result_identity_mismatch", &target));
            checks.push(model::CheckStatus::Failed);
            continue;
        }
        if result
            .output_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
            || (matches!(result.status, Status::RequiresApproval) && !matches!(target.check, 3 | 5))
        {
            reasons.push(Reason::check("invalid_result", &target));
            checks.push(model::CheckStatus::Failed);
            continue;
        }
        *status = Some(result.status);
    }

    let mut provenance_approval = false;
    for (target, status) in targets {
        let (state, reason) = match status {
            Some(Status::Passed) => (model::CheckStatus::Passed, None),
            Some(Status::Failed) => (model::CheckStatus::Failed, Some("check_failed")),
            Some(Status::RequiresApproval) => {
                provenance_approval = true;
                (
                    model::CheckStatus::RequiresApproval,
                    Some("check_requires_approval"),
                )
            }
            None | Some(Status::Incomplete) => {
                let code = if status.is_some() {
                    "check_incomplete"
                } else {
                    "missing_result"
                };
                incomplete_checks.insert(target.clone());
                (model::CheckStatus::Incomplete, Some(code))
            }
        };
        checks.push(state);
        if let Some(code) = reason {
            reasons.push(Reason::check(code, &target));
        }
    }

    let decision = match model::decide_merge(kind, checks) {
        model::Decision::AutoMerge => "auto_merge",
        model::Decision::RequiresApproval => "requires_approval",
        model::Decision::Reject => "reject",
        model::Decision::Incomplete => "incomplete",
    };
    let change_class = if repository_change {
        "repository_change"
    } else if matches!(kind, model::ChangeClass::ApprovalChange) || provenance_approval {
        "approval_change"
    } else {
        "record_change"
    };
    reasons.sort();
    reasons.dedup();

    Ok(Report {
        schema_version: request.schema_version,
        base_sha: request.identity.base_sha,
        head_sha: request.identity.head_sha,
        tree_oid: request.identity.tree_oid,
        policy_digest: request.identity.policy_digest,
        change_class,
        scope,
        decision,
        affected_packages: packages,
        reasons,
        incomplete_checks,
    })
}

fn inspect(
    entry: &DiffEntry,
    core_packages: &BTreeSet<String>,
    packages: &mut BTreeSet<String>,
    reasons: &mut Vec<Reason>,
) -> Result<model::Change, String> {
    let mut change = model::Change {
        path: entry.path.clone(),
        source_changed: false,
        version_deleted: false,
        policy_approval: false,
    };
    let parts: Vec<_> = entry.path.split('/').collect();
    if !entry.path.starts_with("packages/") {
        reasons.push(Reason::path("outside_packages", &entry.path));
        return Ok(change);
    }
    let mut flag = |code| {
        change.policy_approval = true;
        reasons.push(Reason::path(code, &entry.path));
    };
    let ["packages", shard, package, rest @ ..] = parts.as_slice() else {
        flag("unrecognized_package_path");
        return Ok(change);
    };
    if !naming::model::valid_name(package)
        || naming::model::shard(package) != *shard
        || rest.is_empty()
    {
        flag("unrecognized_package_path");
        return Ok(change);
    }
    packages.insert((*package).into());
    if core_packages.contains(*package) {
        flag("core_package");
    }
    if entry.blobs().any(|blob| blob.mode == "100755") {
        flag("executable_package_file");
    }
    if entry.has_unsupported_mode() {
        change.policy_approval = true;
        return Ok(change);
    }

    match rest {
        ["package.toml"] => {
            let (before, after) = record_sides(entry)?;
            if let Some(before) = &before
                && before.get("upstream") != after.as_ref().and_then(|r| r.get("upstream"))
            {
                flag("upstream_changed");
            }
        }
        ["versions", file]
            if file
                .strip_suffix(".toml")
                .is_some_and(naming::model::valid_version) =>
        {
            let (before, after) = record_sides(entry)?;
            if let Some(after) = &after {
                if after.get("build").is_some_and(|build| {
                    build.get("kind").and_then(toml::Value::as_str) == Some("commands")
                        || build
                            .get("configure-command")
                            .and_then(toml::Value::as_array)
                            .is_some_and(|command| !command.is_empty())
                }) {
                    flag("arbitrary_build_command");
                }
                if before
                    .as_ref()
                    .is_some_and(|before| tests_weakened(before, after))
                {
                    flag("tests_disabled");
                }
                change.source_changed = before
                    .as_ref()
                    .is_some_and(|before| source_table(before) != source_table(after));
                if change.source_changed {
                    reasons.push(Reason::path("existing_source_changed", &entry.path));
                }
            }
            change.version_deleted = after.is_none();
            if change.version_deleted {
                reasons.push(Reason::path("version_deleted", &entry.path));
            }
        }
        ["patches", file] if file.ends_with(".patch") => {}
        _ => flag("unrecognized_package_path"),
    }
    Ok(change)
}

fn record_sides(entry: &DiffEntry) -> Result<(Option<toml::Table>, Option<toml::Table>), String> {
    let parse = |blob: &Blob| {
        let text = blob
            .text
            .as_deref()
            .ok_or_else(|| format!("{}: missing record text", entry.path))?;
        text.parse().map_err(|e| format!("{}: {e}", entry.path))
    };
    Ok((
        entry.before.as_ref().map(parse).transpose()?,
        entry.after.as_ref().map(parse).transpose()?,
    ))
}

fn source_table(record: &toml::Table) -> Option<toml::Table> {
    let mut table = record.get("source")?.as_table()?.clone();
    table
        .entry("strip-components")
        .or_insert(toml::Value::Integer(1));
    Some(table)
}

fn tests_weakened(before: &toml::Table, after: &toml::Table) -> bool {
    fn field<'a>(record: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
        record.get("check")?.get(key)
    }
    let newly_disabled = field(before, "enabled").and_then(toml::Value::as_bool) == Some(true)
        && field(after, "enabled").and_then(toml::Value::as_bool) == Some(false);
    let before_skip = field(before, "skip").and_then(toml::Value::as_array);
    let newly_skipped = field(after, "skip")
        .and_then(toml::Value::as_array)
        .is_some_and(|skip| {
            skip.iter()
                .any(|test| before_skip.is_none_or(|old| !old.contains(test)))
        });
    newly_disabled || newly_skipped
}
