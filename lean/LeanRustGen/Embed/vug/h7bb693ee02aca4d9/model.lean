namespace model

inductive Component where
  | num (value : Nat)
  | text (value : String)
  deriving DecidableEq, Repr

inductive RunKind where
  | digits
  | other
  deriving DecidableEq, Repr, BEq

def isSeparator (c : Char) : Bool :=
  c == '.' || c == '-' || c == '_' || c == '+'

def digitsToNat (cs : List Char) : Nat :=
  cs.foldl (fun n c => n * 10 + (c.toNat - '0'.toNat)) 0

def runComponent (kind : RunKind) (cs : List Char) : Component :=
  match kind with
  | .digits => .num (digitsToNat cs)
  | .other => .text (String.ofList cs)

def flush (out : List Component) (kind : Option RunKind) (acc : List Char) : List Component :=
  match kind with
  | none => out
  | some k => out ++ [runComponent k acc.reverse]

def parseLoop (out : List Component) (kind : Option RunKind) (acc : List Char) :
    List Char → List Component
  | [] => flush out kind acc
  | c :: cs =>
    if isSeparator c then
      parseLoop (flush out kind acc) none [] cs
    else
      let k := if c.isDigit then RunKind.digits else RunKind.other
      match kind with
      | none => parseLoop out (some k) [c] cs
      | some k' =>
        if k' == k then parseLoop out kind (c :: acc) cs
        else parseLoop (flush out kind acc) (some k) [c] cs

def parse (s : String) : List Component := parseLoop [] none [] s.toList

def cmpNat (a b : Nat) : Ordering :=
  if a < b then .lt else if a = b then .eq else .gt

def flipOrd : Ordering → Ordering
  | .lt => .gt
  | .eq => .eq
  | .gt => .lt

def compareChars : List Char → List Char → Ordering
  | [], [] => .eq
  | [], _ :: _ => .lt
  | _ :: _, [] => .gt
  | a :: as, b :: bs =>
    match cmpNat a.toNat b.toNat with
    | .eq => compareChars as bs
    | o => o

def compareList : List Component → List Component → Ordering
  | [], [] => .eq
  | [], .num _ :: _ => .lt
  | [], .text _ :: _ => .gt
  | .num _ :: _, [] => .gt
  | .text _ :: _, [] => .lt
  | .num a :: xs, .num b :: ys =>
    match cmpNat a b with
    | .eq => compareList xs ys
    | o => o
  | .text a :: xs, .text b :: ys =>
    match compareChars a.toList b.toList with
    | .eq => compareList xs ys
    | o => o
  | .num _ :: _, .text _ :: _ => .gt
  | .text _ :: _, .num _ :: _ => .lt

def compare (a b : String) : Ordering :=
  compareList (parse a) (parse b)

def usageStatus : Nat := 252

def failureStatus : Nat := 254

def otherStatus : Nat := 255

theorem cmpNat_eq_iff (a b : Nat) : cmpNat a b = .eq ↔ a = b := by
  unfold cmpNat
  constructor
  · intro h
    by_cases hlt : a < b
    · simp [hlt] at h
    · by_cases heq : a = b
      · exact heq
      · simp [hlt, heq] at h
  · intro h
    subst h
    simp

theorem cmpNat_lt_iff (a b : Nat) : cmpNat a b = .lt ↔ a < b := by
  unfold cmpNat
  constructor
  · intro h
    by_cases hlt : a < b
    · exact hlt
    · by_cases heq : a = b
      · simp [heq] at h
      · simp [hlt, heq] at h
  · intro h
    simp [h]

theorem cmpNat_gt_iff (a b : Nat) : cmpNat a b = .gt ↔ b < a := by
  unfold cmpNat
  constructor
  · intro h
    by_cases hlt : a < b
    · simp [hlt] at h
    · by_cases heq : a = b
      · simp [heq] at h
      · simp [hlt, heq] at h
        omega
  · intro h
    have h1 : ¬ a < b := by omega
    have h2 : a ≠ b := by omega
    simp [h1, h2]

theorem cmpNat_flip (a b : Nat) : cmpNat b a = flipOrd (cmpNat a b) := by
  by_cases hlt : a < b
  · rw [(cmpNat_lt_iff a b).mpr hlt, (cmpNat_gt_iff b a).mpr hlt]
    rfl
  · by_cases heq : a = b
    · subst heq
      rw [(cmpNat_eq_iff a a).mpr rfl]
      rfl
    · have hgt : b < a := by omega
      rw [(cmpNat_gt_iff a b).mpr hgt, (cmpNat_lt_iff b a).mpr hgt]
      rfl

theorem compareChars_eq_iff (a b : List Char) :
    compareChars a b = .eq ↔ a = b := by
  induction a generalizing b with
  | nil => cases b <;> simp [compareChars]
  | cons x xs ih =>
    cases b with
    | nil => simp [compareChars]
    | cons y ys =>
      simp only [compareChars]
      by_cases hxy : x = y
      · subst hxy
        rw [(cmpNat_eq_iff x.toNat x.toNat).mpr rfl]
        simp [ih]
      · cases h : cmpNat x.toNat y.toNat
        · simp [hxy]
        · exact absurd (Char.toNat_inj.mp ((cmpNat_eq_iff x.toNat y.toNat).mp h)) hxy
        · simp [hxy]

theorem compareChars_flip (a b : List Char) :
    compareChars b a = flipOrd (compareChars a b) := by
  induction a generalizing b with
  | nil => cases b <;> simp [compareChars, flipOrd]
  | cons x xs ih =>
    cases b with
    | nil => simp [compareChars, flipOrd]
    | cons y ys =>
      simp only [compareChars]
      by_cases hxy : x = y
      · subst hxy
        rw [(cmpNat_eq_iff x.toNat x.toNat).mpr rfl]
        exact ih ys
      · cases h : cmpNat x.toNat y.toNat
        · have hlt := (cmpNat_lt_iff x.toNat y.toNat).mp h
          rw [(cmpNat_gt_iff y.toNat x.toNat).mpr hlt]
          rfl
        · exact absurd (Char.toNat_inj.mp ((cmpNat_eq_iff x.toNat y.toNat).mp h)) hxy
        · have hgt := (cmpNat_gt_iff x.toNat y.toNat).mp h
          rw [(cmpNat_lt_iff y.toNat x.toNat).mpr hgt]
          rfl

theorem compareChars_lt_trans (a b c : List Char) :
    compareChars a b = .lt → compareChars b c = .lt → compareChars a c = .lt := by
  induction a generalizing b c with
  | nil =>
    intro h1 h2
    cases b with
    | nil => simp [compareChars] at h1
    | cons y ys =>
      cases c with
      | nil => simp [compareChars] at h2
      | cons z zs => rfl
  | cons x xs ih =>
    intro h1 h2
    cases b with
    | nil => simp [compareChars] at h1
    | cons y ys =>
      cases c with
      | nil => simp [compareChars] at h2
      | cons z zs =>
        simp only [compareChars] at h1 h2 ⊢
        cases hxy : cmpNat x.toNat y.toNat
        · have hxylt := (cmpNat_lt_iff x.toNat y.toNat).mp hxy
          cases hyz : cmpNat y.toNat z.toNat
          · have hyzlt := (cmpNat_lt_iff y.toNat z.toNat).mp hyz
            rw [(cmpNat_lt_iff x.toNat z.toNat).mpr (Nat.lt_trans hxylt hyzlt)]
          · have hyze : y = z := Char.toNat_inj.mp ((cmpNat_eq_iff y.toNat z.toNat).mp hyz)
            rw [(cmpNat_lt_iff x.toNat z.toNat).mpr (hyze ▸ hxylt)]
          · rw [hyz] at h2
            cases h2
        · have hxeq : x = y := Char.toNat_inj.mp ((cmpNat_eq_iff x.toNat y.toNat).mp hxy)
          rw [hxy] at h1
          cases hyz : cmpNat y.toNat z.toNat
          · have hyzlt := (cmpNat_lt_iff y.toNat z.toNat).mp hyz
            rw [(cmpNat_lt_iff x.toNat z.toNat).mpr (hxeq.symm ▸ hyzlt)]
          · have hyze : y = z := Char.toNat_inj.mp ((cmpNat_eq_iff y.toNat z.toNat).mp hyz)
            rw [(cmpNat_eq_iff x.toNat z.toNat).mpr (congrArg Char.toNat (hxeq.trans hyze))]
            rw [hyz] at h2
            exact ih ys zs h1 h2
          · rw [hyz] at h2
            cases h2
        · rw [hxy] at h1
          cases h1

theorem compareList_eq_iff (xs ys : List Component) :
    compareList xs ys = .eq ↔ xs = ys := by
  induction xs generalizing ys with
  | nil =>
    cases ys with
    | nil => simp [compareList]
    | cons y ys => cases y <;> simp [compareList]
  | cons x xs ih =>
    cases ys with
    | nil => cases x <;> simp [compareList]
    | cons y ys =>
      cases x with
      | num a =>
        cases y with
        | num b =>
          simp only [compareList]
          cases h : cmpNat a b
          · have hlt := (cmpNat_lt_iff a b).mp h
            simp
            intro hab
            exact absurd hab (by omega)
          · have heq := (cmpNat_eq_iff a b).mp h
            subst heq
            simp [ih]
          · have hgt := (cmpNat_gt_iff a b).mp h
            simp
            intro hab
            exact absurd hab (by omega)
        | text t => simp [compareList]
      | text s =>
        cases y with
        | num b => simp [compareList]
        | text t =>
          simp only [compareList]
          cases h : compareChars s.toList t.toList
          · have hne : s ≠ t := by
              intro he
              rw [he] at h
              rw [(compareChars_eq_iff t.toList t.toList).mpr rfl] at h
              cases h
            simp [hne]
          · have heq : s = t :=
              String.toList_injective ((compareChars_eq_iff s.toList t.toList).mp h)
            subst heq
            simp [ih]
          · have hne : s ≠ t := by
              intro he
              rw [he] at h
              rw [(compareChars_eq_iff t.toList t.toList).mpr rfl] at h
              cases h
            simp [hne]

theorem compareList_flip (xs ys : List Component) :
    compareList ys xs = flipOrd (compareList xs ys) := by
  induction xs generalizing ys with
  | nil =>
    cases ys with
    | nil => rfl
    | cons y ys => cases y <;> rfl
  | cons x xs ih =>
    cases ys with
    | nil => cases x <;> rfl
    | cons y ys =>
      cases x with
      | num a =>
        cases y with
        | num b =>
          simp only [compareList]
          rw [cmpNat_flip a b]
          cases h : cmpNat a b
          · rfl
          · exact ih ys
          · rfl
        | text t => rfl
      | text s =>
        cases y with
        | num b => rfl
        | text t =>
          simp only [compareList]
          rw [compareChars_flip s.toList t.toList]
          cases h : compareChars s.toList t.toList
          · rfl
          · exact ih ys
          · rfl

theorem compareList_lt_trans (xs ys zs : List Component) :
    compareList xs ys = .lt → compareList ys zs = .lt → compareList xs zs = .lt := by
  induction xs generalizing ys zs with
  | nil =>
    intro h1 h2
    cases ys with
    | nil => simp [compareList] at h1
    | cons y ys =>
      cases y with
      | num n =>
        cases zs with
        | nil => simp [compareList] at h2
        | cons z zs =>
          cases z with
          | num m => rfl
          | text t => simp [compareList] at h2
      | text s => simp [compareList] at h1
  | cons x xs ih =>
    intro h1 h2
    cases ys with
    | nil =>
      cases x with
      | num n => simp [compareList] at h1
      | text s =>
        cases zs with
        | nil => simp [compareList] at h2
        | cons z zs =>
          cases z with
          | num m => rfl
          | text t => simp [compareList] at h2
    | cons y ys =>
      cases zs with
      | nil =>
        cases y with
        | num n => simp [compareList] at h2
        | text t =>
          cases x with
          | num n => simp [compareList] at h1
          | text s => rfl
      | cons z zs =>
        cases x with
        | num nx =>
          cases y with
          | num ny =>
            cases z with
            | num nz =>
              simp only [compareList] at h1 h2 ⊢
              cases hxy : cmpNat nx ny
              · have hxylt := (cmpNat_lt_iff nx ny).mp hxy
                cases hyz : cmpNat ny nz
                · have hyzlt := (cmpNat_lt_iff ny nz).mp hyz
                  rw [(cmpNat_lt_iff nx nz).mpr (Nat.lt_trans hxylt hyzlt)]
                · have hyze : ny = nz := (cmpNat_eq_iff ny nz).mp hyz
                  rw [(cmpNat_lt_iff nx nz).mpr (hyze ▸ hxylt)]
                · rw [hyz] at h2
                  cases h2
              · have hxeq : nx = ny := (cmpNat_eq_iff nx ny).mp hxy
                rw [hxy] at h1
                cases hyz : cmpNat ny nz
                · have hyzlt := (cmpNat_lt_iff ny nz).mp hyz
                  rw [(cmpNat_lt_iff nx nz).mpr (hxeq.symm ▸ hyzlt)]
                · have hyze : ny = nz := (cmpNat_eq_iff ny nz).mp hyz
                  rw [(cmpNat_eq_iff nx nz).mpr (hxeq.trans hyze)]
                  rw [hyz] at h2
                  exact ih ys zs h1 h2
                · rw [hyz] at h2
                  cases h2
              · rw [hxy] at h1
                cases h1
            | text tz => simp [compareList] at h2
          | text ty =>
            cases z <;> simp [compareList] at h1
        | text tx =>
          cases y with
          | num ny =>
            cases z with
            | num nz => rfl
            | text tz => simp [compareList] at h2
          | text ty =>
            cases z with
            | num nz => rfl
            | text tz =>
              simp only [compareList] at h1 h2 ⊢
              cases hxy : compareChars tx.toList ty.toList
              · cases hyz : compareChars ty.toList tz.toList
                · rw [compareChars_lt_trans _ _ _ hxy hyz]
                · have hyze : ty = tz :=
                    String.toList_injective ((compareChars_eq_iff _ _).mp hyz)
                  rw [(hyze ▸ hxy : compareChars tx.toList tz.toList = .lt)]
                · rw [hyz] at h2
                  cases h2
              · have hxeq : tx = ty :=
                    String.toList_injective ((compareChars_eq_iff _ _).mp hxy)
                rw [hxy] at h1
                cases hyz : compareChars ty.toList tz.toList
                · rw [(hxeq.symm ▸ hyz : compareChars tx.toList tz.toList = .lt)]
                · have hyze : ty = tz :=
                    String.toList_injective ((compareChars_eq_iff _ _).mp hyz)
                  rw [(compareChars_eq_iff tx.toList tz.toList).mpr
                    (congrArg String.toList (hxeq.trans hyze))]
                  rw [hyz] at h2
                  exact ih ys zs h1 h2
                · rw [hyz] at h2
                  cases h2
              · rw [hxy] at h1
                cases h1

theorem compare_refl (a : String) : compare a a = .eq :=
  (compareList_eq_iff (parse a) (parse a)).mpr rfl

theorem compare_flip (a b : String) : compare b a = flipOrd (compare a b) :=
  compareList_flip (parse a) (parse b)

theorem compare_eq_iff_parse_eq (a b : String) :
    compare a b = .eq ↔ parse a = parse b :=
  compareList_eq_iff (parse a) (parse b)

theorem compare_lt_swap (a b : String) :
    compare a b = .lt ↔ compare b a = .gt := by
  rw [compare_flip a b]
  cases compare a b <;> simp [flipOrd]

theorem compare_gt_swap (a b : String) :
    compare a b = .gt ↔ compare b a = .lt := by
  rw [compare_flip a b]
  cases compare a b <;> simp [flipOrd]

theorem compare_eq_swap (a b : String) :
    compare a b = .eq ↔ compare b a = .eq := by
  rw [compare_flip a b]
  cases compare a b <;> simp [flipOrd]

theorem compare_lt_trans (a b c : String) :
    compare a b = .lt → compare b c = .lt → compare a c = .lt :=
  compareList_lt_trans (parse a) (parse b) (parse c)

end model
