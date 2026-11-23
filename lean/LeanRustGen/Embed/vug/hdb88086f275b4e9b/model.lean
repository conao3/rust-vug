namespace model

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

end model
