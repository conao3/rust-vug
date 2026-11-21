namespace model

def isNameFirstChar (c : Char) : Bool := c.isLower || c.isDigit

def isNameChar (c : Char) : Bool :=
  isNameFirstChar c || c == '+' || c == '.' || c == '_' || c == '-'

def isVersionFirstChar (c : Char) : Bool := c.isAlphanum

def isVersionChar (c : Char) : Bool :=
  isVersionFirstChar c || c == '+' || c == '.' || c == '_' || c == '~' || c == '-'

def tomlSuffix : List Char := ['.', 't', 'o', 'm', 'l']

def endsWithToml (l : List Char) : Bool := tomlSuffix.isSuffixOf l

def nameCharsOk : List Char → Bool
  | [] => false
  | c :: cs => isNameFirstChar c && cs.all isNameChar

def versionCharsOk : List Char → Bool
  | [] => false
  | c :: cs => isVersionFirstChar c && cs.all isVersionChar

def validName (s : String) : Bool :=
  let l := s.toList
  nameCharsOk l && l.length ≤ 64 && l != ['.'] && l != ['.', '.'] && !endsWithToml l

def validVersion (s : String) : Bool :=
  let l := s.toList
  versionCharsOk l && l.length ≤ 64 && l != ['.'] && l != ['.', '.'] && !endsWithToml l

def shard (name : String) : String :=
  match name.toList with
  | c :: d :: _ => String.ofList [c, d]
  | [c] => String.ofList [c, '_']
  | [] => ""

def packageDir (name : String) : String :=
  "packages/" ++ shard name ++ "/" ++ name

def versionPath (name version : String) : String :=
  packageDir name ++ "/versions/" ++ version ++ ".toml"

-- helpers

theorem isLower_toNat {c : Char} (h : c.isLower = true) :
    97 ≤ c.toNat ∧ c.toNat ≤ 122 := by
  have h0 : (decide (c.val ≥ 'a'.val) && decide (c.val ≤ 'z'.val)) = true := h
  rw [Bool.and_eq_true, decide_eq_true_eq, decide_eq_true_eq, ge_iff_le] at h0
  obtain ⟨h1, h2⟩ := h0
  exact ⟨UInt32.le_iff_toNat_le.mp h1, UInt32.le_iff_toNat_le.mp h2⟩

theorem isDigit_toNat {c : Char} (h : c.isDigit = true) :
    48 ≤ c.toNat ∧ c.toNat ≤ 57 := by
  have h0 : (decide (c.val ≥ '0'.val) && decide (c.val ≤ '9'.val)) = true := h
  rw [Bool.and_eq_true, decide_eq_true_eq, decide_eq_true_eq, ge_iff_le] at h0
  obtain ⟨h1, h2⟩ := h0
  exact ⟨UInt32.le_iff_toNat_le.mp h1, UInt32.le_iff_toNat_le.mp h2⟩

theorem isUpper_toNat {c : Char} (h : c.isUpper = true) :
    65 ≤ c.toNat ∧ c.toNat ≤ 90 := by
  have h0 : decide (c.val ≥ 'A'.val ∧ c.val ≤ 'Z'.val) = true := h
  rw [decide_eq_true_eq] at h0
  obtain ⟨h1, h2⟩ := h0
  exact ⟨UInt32.le_iff_toNat_le.mp h1, UInt32.le_iff_toNat_le.mp h2⟩

theorem isAlpha_toNat {c : Char} (h : c.isAlpha = true) :
    (65 ≤ c.toNat ∧ c.toNat ≤ 90) ∨ (97 ≤ c.toNat ∧ c.toNat ≤ 122) := by
  have h0 : (c.isUpper || c.isLower) = true := h
  rw [Bool.or_eq_true] at h0
  rcases h0 with h0 | h0
  · exact Or.inl (isUpper_toNat h0)
  · exact Or.inr (isLower_toNat h0)

theorem isAlphanum_toNat {c : Char} (h : c.isAlphanum = true) :
    (65 ≤ c.toNat ∧ c.toNat ≤ 90) ∨ (97 ≤ c.toNat ∧ c.toNat ≤ 122) ∨
    (48 ≤ c.toNat ∧ c.toNat ≤ 57) := by
  have h0 : (c.isAlpha || c.isDigit) = true := h
  rw [Bool.or_eq_true] at h0
  rcases h0 with h0 | h0
  · rcases isAlpha_toNat h0 with h' | h'
    · exact Or.inl h'
    · exact Or.inr (Or.inl h')
  · exact Or.inr (Or.inr (isDigit_toNat h0))

theorem isNameFirstChar_toNat {c : Char} (h : isNameFirstChar c = true) :
    (48 ≤ c.toNat ∧ c.toNat ≤ 57) ∨ (97 ≤ c.toNat ∧ c.toNat ≤ 122) := by
  have h0 : (c.isLower || c.isDigit) = true := h
  rw [Bool.or_eq_true] at h0
  rcases h0 with h0 | h0
  · exact Or.inr (isLower_toNat h0)
  · exact Or.inl (isDigit_toNat h0)

theorem isNameChar_toNat {c : Char} (h : isNameChar c = true) :
    (48 ≤ c.toNat ∧ c.toNat ≤ 57) ∨ (97 ≤ c.toNat ∧ c.toNat ≤ 122) ∨
    c.toNat = 43 ∨ c.toNat = 46 ∨ c.toNat = 95 ∨ c.toNat = 45 := by
  have h0 : ((((isNameFirstChar c || c == '+') || c == '.') || c == '_') || c == '-') = true := h
  rw [Bool.or_eq_true, Bool.or_eq_true, Bool.or_eq_true, Bool.or_eq_true, beq_iff_eq,
    beq_iff_eq, beq_iff_eq, beq_iff_eq] at h0
  rcases h0 with ((((h0 | h0) | h0) | h0) | h0)
  · rcases isNameFirstChar_toNat h0 with h' | h'
    · exact Or.inl h'
    · exact Or.inr (Or.inl h')
  · exact Or.inr (Or.inr (Or.inl (by rw [h0]; rfl)))
  · exact Or.inr (Or.inr (Or.inr (Or.inl (by rw [h0]; rfl))))
  · exact Or.inr (Or.inr (Or.inr (Or.inr (Or.inl (by rw [h0]; rfl)))))
  · exact Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (by rw [h0]; rfl)))))

theorem isNameChar_ne_slash {c : Char} (h : isNameChar c = true) : c ≠ '/' := by
  have ht := isNameChar_toNat h
  intro he
  subst he
  simp at ht

theorem isNameChar_not_upper {c : Char} (h : isNameChar c = true) : c.isUpper = false := by
  have ht := isNameChar_toNat h
  have h0 : decide (c.val ≥ 'A'.val ∧ c.val ≤ 'Z'.val) = false := by
    rw [decide_eq_false_iff_not]
    rintro ⟨h1, h2⟩
    have h1' : 65 ≤ c.val.toNat := UInt32.le_iff_toNat_le.mp h1
    have h2' : c.val.toNat ≤ 90 := UInt32.le_iff_toNat_le.mp h2
    rw [show c.toNat = c.val.toNat from rfl] at ht
    omega
  exact h0

theorem isVersionChar_toNat {c : Char} (h : isVersionChar c = true) :
    (65 ≤ c.toNat ∧ c.toNat ≤ 90) ∨ (97 ≤ c.toNat ∧ c.toNat ≤ 122) ∨
    (48 ≤ c.toNat ∧ c.toNat ≤ 57) ∨
    c.toNat = 43 ∨ c.toNat = 46 ∨ c.toNat = 95 ∨ c.toNat = 126 ∨ c.toNat = 45 := by
  have h0 : (((((c.isAlphanum || c == '+') || c == '.') || c == '_') || c == '~') || c == '-') = true := h
  rw [Bool.or_eq_true, Bool.or_eq_true, Bool.or_eq_true, Bool.or_eq_true, Bool.or_eq_true,
    beq_iff_eq, beq_iff_eq, beq_iff_eq, beq_iff_eq, beq_iff_eq] at h0
  rcases h0 with (((((h0 | h0) | h0) | h0) | h0) | h0)
  · rcases isAlphanum_toNat h0 with h' | h' | h'
    · exact Or.inl h'
    · exact Or.inr (Or.inl h')
    · exact Or.inr (Or.inr (Or.inl h'))
  · exact Or.inr (Or.inr (Or.inr (Or.inl (by rw [h0]; rfl))))
  · exact Or.inr (Or.inr (Or.inr (Or.inr (Or.inl (by rw [h0]; rfl)))))
  · exact Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inl (by rw [h0]; rfl))))))
  · exact Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inl (by rw [h0]; rfl)))))))
  · exact Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (by rw [h0]; rfl)))))))

theorem isVersionChar_ne_slash {c : Char} (h : isVersionChar c = true) : c ≠ '/' := by
  have ht := isVersionChar_toNat h
  intro he
  subst he
  simp at ht

theorem isNameFirstChar_isNameChar {c : Char} (h : isNameFirstChar c = true) :
    isNameChar c = true := by
  have e : isNameChar c = ((((isNameFirstChar c || c == '+') || c == '.') || c == '_') || c == '-') := rfl
  rw [e, h]
  simp

theorem isVersionFirstChar_isVersionChar {c : Char} (h : isVersionFirstChar c = true) :
    isVersionChar c = true := by
  have e : isVersionChar c = (((((isVersionFirstChar c || c == '+') || c == '.') || c == '_') || c == '~') || c == '-') := rfl
  rw [e, h]
  simp

theorem nameCharsOk_of_valid {l : List Char} (h : nameCharsOk l = true) :
    ∃ c cs, l = c :: cs ∧ isNameFirstChar c = true ∧ cs.all isNameChar = true := by
  cases l with
  | nil =>
    have e : nameCharsOk [] = false := rfl
    rw [e] at h
    simp at h
  | cons c cs =>
    have e : nameCharsOk (c :: cs) = (isNameFirstChar c && cs.all isNameChar) := rfl
    rw [e, Bool.and_eq_true] at h
    exact ⟨c, cs, rfl, h⟩

theorem versionCharsOk_of_valid {l : List Char} (h : versionCharsOk l = true) :
    ∃ c cs, l = c :: cs ∧ isVersionFirstChar c = true ∧ cs.all isVersionChar = true := by
  cases l with
  | nil =>
    have e : versionCharsOk [] = false := rfl
    rw [e] at h
    simp at h
  | cons c cs =>
    have e : versionCharsOk (c :: cs) = (isVersionFirstChar c && cs.all isVersionChar) := rfl
    rw [e, Bool.and_eq_true] at h
    exact ⟨c, cs, rfl, h⟩

theorem validName_toList {s : String} (h : validName s = true) :
    ∀ c ∈ s.toList, isNameChar c = true := by
  have h0 : (nameCharsOk s.toList && s.toList.length ≤ 64 && s.toList != ['.'] &&
      s.toList != ['.', '.'] && !endsWithToml s.toList) = true := h
  rw [Bool.and_eq_true] at h0
  obtain ⟨h0, _⟩ := h0
  rw [Bool.and_eq_true] at h0
  obtain ⟨h0, _⟩ := h0
  rw [Bool.and_eq_true] at h0
  obtain ⟨h0, _⟩ := h0
  rw [Bool.and_eq_true] at h0
  obtain ⟨hok, _⟩ := h0
  obtain ⟨c, cs, hl, hfirst, hall⟩ := nameCharsOk_of_valid hok
  rw [hl]
  intro x hx
  rcases List.mem_cons.mp hx with heq | hmem
  · rw [heq]; exact isNameFirstChar_isNameChar hfirst
  · exact (List.all_eq_true.mp hall) x hmem

theorem validVersion_toList {s : String} (h : validVersion s = true) :
    ∀ c ∈ s.toList, isVersionChar c = true := by
  have h0 : (versionCharsOk s.toList && s.toList.length ≤ 64 && s.toList != ['.'] &&
      s.toList != ['.', '.'] && !endsWithToml s.toList) = true := h
  rw [Bool.and_eq_true] at h0
  obtain ⟨h0, _⟩ := h0
  rw [Bool.and_eq_true] at h0
  obtain ⟨h0, _⟩ := h0
  rw [Bool.and_eq_true] at h0
  obtain ⟨h0, _⟩ := h0
  rw [Bool.and_eq_true] at h0
  obtain ⟨hok, _⟩ := h0
  obtain ⟨c, cs, hl, hfirst, hall⟩ := versionCharsOk_of_valid hok
  rw [hl]
  intro x hx
  rcases List.mem_cons.mp hx with heq | hmem
  · rw [heq]; exact isVersionFirstChar_isVersionChar hfirst
  · exact (List.all_eq_true.mp hall) x hmem

theorem validName_no_slash {s : String} (h : validName s = true) :
    '/' ∉ s.toList := by
  intro hm
  exact isNameChar_ne_slash (validName_toList h '/' hm) rfl

theorem validVersion_no_slash {s : String} (h : validVersion s = true) :
    '/' ∉ s.toList := by
  intro hm
  exact isVersionChar_ne_slash (validVersion_toList h '/' hm) rfl

theorem validName_no_upper {s : String} (h : validName s = true) :
    ∀ c ∈ s.toList, c.isUpper = false := by
  intro c hc
  exact isNameChar_not_upper (validName_toList h c hc)

theorem validName_nonempty {s : String} (h : validName s = true) : s ≠ "" := by
  intro he
  subst he
  have e : validName "" = false := rfl
  rw [e] at h
  simp at h

theorem validVersion_nonempty {s : String} (h : validVersion s = true) : s ≠ "" := by
  intro he
  subst he
  have e : validVersion "" = false := rfl
  rw [e] at h
  simp at h

theorem validName_toList_ne_nil {s : String} (h : validName s = true) : s.toList ≠ [] := by
  intro he
  have hs : s = "" := String.toList_injective (by rw [he]; rfl)
  exact validName_nonempty h hs

theorem validVersion_toList_ne_nil {s : String} (h : validVersion s = true) : s.toList ≠ [] := by
  intro he
  have hs : s = "" := String.toList_injective (by rw [he]; rfl)
  exact validVersion_nonempty h hs

theorem shard_toList (s : String) :
    (shard s).toList =
      match s.toList with
      | c :: d :: _ => [c, d]
      | [c] => [c, '_']
      | [] => [] := by
  have e : (shard s).toList =
      (match s.toList with
      | c :: d :: _ => String.ofList [c, d]
      | [c] => String.ofList [c, '_']
      | [] => "").toList := rfl
  rw [e]
  cases s.toList with
  | nil => rfl
  | cons c cs =>
    cases cs with
    | nil => simp
    | cons d ds => simp

theorem shard_length {s : String} (h : s.toList ≠ []) : (shard s).toList.length = 2 := by
  rw [shard_toList]
  cases hs : s.toList with
  | nil => exact absurd hs h
  | cons c cs =>
    cases cs with
    | nil => rfl
    | cons d ds => rfl

theorem packageDir_toList (s : String) :
    (packageDir s).toList =
      "packages/".toList ++ (shard s).toList ++ ['/'] ++ s.toList := by
  have e : packageDir s = ("packages/" ++ shard s ++ "/" ++ s) := rfl
  rw [e, String.toList_append, String.toList_append, String.toList_append]
  rfl

theorem versionPath_toList (n v : String) :
    (versionPath n v).toList =
      (packageDir n).toList ++ "/versions/".toList ++ v.toList ++ ".toml".toList := by
  have e : versionPath n v = (packageDir n ++ "/versions/" ++ v ++ ".toml") := rfl
  rw [e, String.toList_append, String.toList_append, String.toList_append]

theorem packageDir_toList_inj {a b : String} (ha : validName a = true) (hb : validName b = true)
    (h : (packageDir a).toList = (packageDir b).toList) : a = b := by
  have hna := validName_toList_ne_nil ha
  have hnb := validName_toList_ne_nil hb
  rw [packageDir_toList, packageDir_toList] at h
  have h3 : a.toList = b.toList :=
    List.append_inj_right h (by
      simp only [List.length_append, shard_length hna, shard_length hnb])
  exact String.toList_injective h3

theorem packageDir_inj {a b : String} (ha : validName a = true) (hb : validName b = true)
    (h : packageDir a = packageDir b) : a = b :=
  packageDir_toList_inj ha hb (congrArg String.toList h)

-- suffix/prefix slash-freeness of valid names and versions

theorem validName_suffix_no_slash {s : String} (h : validName s = true)
    {t : List Char} (ht : t <:+ s.toList) : ∀ c ∈ t, c ≠ '/' := by
  obtain ⟨u, hu⟩ := ht
  intro c hc he
  have hc' : c ∈ s.toList := by rw [← hu]; exact List.mem_append_right u hc
  exact isNameChar_ne_slash (validName_toList h c hc') he

theorem validName_prefix_no_slash {s : String} (h : validName s = true)
    {t : List Char} (ht : t <+: s.toList) : ∀ c ∈ t, c ≠ '/' := by
  obtain ⟨u, hu⟩ := ht
  intro c hc he
  have hc' : c ∈ s.toList := by rw [← hu]; exact List.mem_append_left u hc
  exact isNameChar_ne_slash (validName_toList h c hc') he

theorem validVersion_prefix_no_slash {s : String} (h : validVersion s = true)
    {t : List Char} (ht : t <+: s.toList) : ∀ c ∈ t, c ≠ '/' := by
  obtain ⟨u, hu⟩ := ht
  intro c hc he
  have hc' : c ∈ s.toList := by rw [← hu]; exact List.mem_append_left u hc
  exact isVersionChar_ne_slash (validVersion_toList h c hc') he

-- a prefix of a cons-list headed by '/' is empty or contains '/'

theorem prefix_slash_mem {r t u : List Char} (h : '/' :: r = t ++ u) (hne : t ≠ []) :
    '/' ∈ t := by
  cases t with
  | nil => exact absurd rfl hne
  | cons c cs =>
    have hc : c = '/' := (List.cons.inj h).1.symm
    rw [hc]
    exact List.mem_cons_self

-- suffix dichotomy over an appended split

theorem suffix_append_cases {s l₁ l₂ : List Char} (h : s <:+ l₁ ++ l₂) :
    s <:+ l₂ ∨ l₂ <:+ s := by
  obtain ⟨t, ht⟩ := h
  rw [List.append_eq_append_iff] at ht
  rcases ht with ⟨u, h1, h2⟩ | ⟨u, h1, h2⟩
  · exact Or.inr ⟨u, h2.symm⟩
  · exact Or.inl ⟨u, h2.symm⟩

theorem prefixLen {s : String} (h : s.toList ≠ []) :
    ("packages/".toList ++ (shard s).toList ++ ['/']).length = 12 := by
  rw [List.length_append, List.length_append, shard_length h]
  rfl

-- the tail of a longer package directory is a suffix of the longer name

theorem packageDir_suffix {a b : String} {t : List Char}
    (ha : a.toList ≠ []) (hb : b.toList ≠ [])
    (h : (packageDir b).toList = (packageDir a).toList ++ t) : t <:+ b.toList := by
  rw [packageDir_toList, packageDir_toList] at h
  rw [List.append_eq_append_iff] at h
  rcases h with ⟨u, h1, h2⟩ | ⟨u, h1, h2⟩
  · -- h1 : Xa ++ a.tl = Xb ++ u ; h2 : b.tl = u ++ t
    rw [List.append_eq_append_iff] at h1
    rcases h1 with ⟨v, hv1, hv2⟩ | ⟨v, hv1, hv2⟩
    · -- hv1 : Xb = Xa ++ v ; hv2 : a.tl = v ++ u
      have hv0 : v = [] := by
        have hl := congrArg List.length hv1
        simp only [List.length_append, shard_length hb, shard_length ha] at hl
        exact List.eq_nil_of_length_eq_zero (by omega)
      subst hv0
      rw [List.nil_append] at hv2
      rw [← hv2] at h2
      exact ⟨a.toList, h2.symm⟩
    · -- hv1 : Xa = Xb ++ v ; hv2 : u = v ++ a.tl
      have hv0 : v = [] := by
        have hl := congrArg List.length hv1
        simp only [List.length_append, shard_length ha, shard_length hb] at hl
        exact List.eq_nil_of_length_eq_zero (by omega)
      subst hv0
      rw [List.nil_append] at hv2
      rw [hv2] at h2
      exact ⟨a.toList, h2.symm⟩
  · -- h1 : Xb = (Xa ++ a.tl) ++ u — impossible: |Xb| = 12 < 12 + |a.tl|
    have hl := congrArg List.length h1
    simp only [List.length_append, shard_length ha, shard_length hb] at hl
    have h0 : a.toList = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact absurd h0 ha

theorem versionPath_inj {a b v w : String}
    (ha : validName a = true) (hb : validName b = true)
    (hv : validVersion v = true) (hw : validVersion w = true)
    (h : versionPath a v = versionPath b w) : a = b ∧ v = w := by
  have hna := validName_toList_ne_nil ha
  have hnb := validName_toList_ne_nil hb
  have heq : (packageDir a).toList ++ "/versions/".toList ++ v.toList ++ ".toml".toList =
      (packageDir b).toList ++ "/versions/".toList ++ w.toList ++ ".toml".toList := by
    have h0 := congrArg String.toList h
    rwa [versionPath_toList, versionPath_toList] at h0
  rw [List.append_left_inj ".toml".toList] at heq
  rw [List.append_eq_append_iff] at heq
  rcases heq with ⟨t, h1, h2⟩ | ⟨t, h1, h2⟩
  · -- h1 : PDb ++ S = (PDa ++ S) ++ t ; h2 : v.tl = t ++ w.tl
    have hslash : ∀ c ∈ t, c ≠ '/' := validVersion_prefix_no_slash hv ⟨w.toList, h2.symm⟩
    rw [List.append_assoc] at h1
    rw [List.append_eq_append_iff] at h1
    rcases h1 with ⟨u, hu1, hu2⟩ | ⟨u, hu1, hu2⟩
    · -- hu1 : PDa = PDb ++ u ; hu2 : S = u ++ S ++ t
      have hl := congrArg List.length hu2
      simp only [List.length_append] at hl
      have hu0 : u = [] := List.eq_nil_of_length_eq_zero (by omega)
      have ht0 : t = [] := List.eq_nil_of_length_eq_zero (by omega)
      subst hu0
      rw [List.append_nil] at hu1
      subst ht0
      rw [List.nil_append] at h2
      exact ⟨packageDir_toList_inj ha hb hu1, String.toList_injective h2⟩
    · -- hu1 : PDb = PDa ++ u ; hu2 : S ++ t = u ++ S
      have usuf : u <:+ b.toList := packageDir_suffix hna hnb hu1
      have unoslash := validName_suffix_no_slash hb usuf
      have upre : u <+: "/versions/".toList ++ t := ⟨"/versions/".toList, hu2.symm⟩
      have hu0 : u = [] := by
        by_cases hne : u = []
        · exact hne
        · have hmem : '/' ∈ u := by
            obtain ⟨r, hr⟩ := upre
            have hcons : '/' :: ("versions/".toList ++ t) = u ++ r := hr.symm
            exact prefix_slash_mem hcons hne
          exact absurd rfl (unoslash '/' hmem)
      subst hu0
      rw [List.append_nil] at hu1
      rw [List.nil_append] at hu2
      have ht0 : t = [] := (List.append_right_inj "/versions/".toList).mp
        (by rw [List.append_nil]; exact hu2)
      subst ht0
      rw [List.nil_append] at h2
      exact ⟨packageDir_toList_inj ha hb hu1.symm, String.toList_injective h2⟩
  · -- h1 : PDa ++ S = (PDb ++ S) ++ t ; h2 : w.tl = t ++ v.tl
    have hslash : ∀ c ∈ t, c ≠ '/' := validVersion_prefix_no_slash hw ⟨v.toList, h2.symm⟩
    rw [List.append_assoc] at h1
    rw [List.append_eq_append_iff] at h1
    rcases h1 with ⟨u, hu1, hu2⟩ | ⟨u, hu1, hu2⟩
    · -- hu1 : PDb = PDa ++ u ; hu2 : S = u ++ S ++ t
      have hl := congrArg List.length hu2
      simp only [List.length_append] at hl
      have hu0 : u = [] := List.eq_nil_of_length_eq_zero (by omega)
      have ht0 : t = [] := List.eq_nil_of_length_eq_zero (by omega)
      subst hu0
      rw [List.append_nil] at hu1
      subst ht0
      rw [List.nil_append] at h2
      exact ⟨packageDir_toList_inj ha hb hu1.symm, (String.toList_injective h2).symm⟩
    · -- hu1 : PDa = PDb ++ u ; hu2 : S ++ t = u ++ S
      have usuf : u <:+ a.toList := packageDir_suffix hnb hna hu1
      have unoslash := validName_suffix_no_slash ha usuf
      have upre : u <+: "/versions/".toList ++ t := ⟨"/versions/".toList, hu2.symm⟩
      have hu0 : u = [] := by
        by_cases hne : u = []
        · exact hne
        · have hmem : '/' ∈ u := by
            obtain ⟨r, hr⟩ := upre
            have hcons : '/' :: ("versions/".toList ++ t) = u ++ r := hr.symm
            exact prefix_slash_mem hcons hne
          exact absurd rfl (unoslash '/' hmem)
      subst hu0
      rw [List.append_nil] at hu1
      rw [List.nil_append] at hu2
      have ht0 : t = [] := (List.append_right_inj "/versions/".toList).mp
        (by rw [List.append_nil]; exact hu2)
      subst ht0
      rw [List.nil_append] at h2
      exact ⟨packageDir_toList_inj ha hb hu1, (String.toList_injective h2).symm⟩

theorem packageDir_no_prefix {a b v : String}
    (ha : validName a = true) (hb : validName b = true) (_hv : validVersion v = true)
    (hne : a ≠ b) : ¬ ((packageDir a ++ "/").toList <+: (versionPath b v).toList) := by
  rintro ⟨u, hu⟩
  have hna := validName_toList_ne_nil ha
  have hnb := validName_toList_ne_nil hb
  have hu' : (versionPath b v).toList = (packageDir a ++ "/").toList ++ u := hu.symm
  rw [versionPath_toList, String.toList_append] at hu'
  conv at hu' => lhs; rw [List.append_assoc, List.append_assoc]
  conv at hu' => rhs; rw [List.append_assoc]
  have hu'' : (packageDir b).toList ++ ("/versions/".toList ++
      (v.toList ++ ".toml".toList)) = (packageDir a).toList ++ '/' :: u := hu'
  rw [List.append_eq_append_iff] at hu''
  rcases hu'' with ⟨t, h1, h2⟩ | ⟨t, h1, h2⟩
  · -- h1 : PDa = PDb ++ t ; h2 : S ++ (v.tl ++ T) = t ++ '/' :: u
    have tsuf : t <:+ a.toList := packageDir_suffix hnb hna h1
    have tnoslash := validName_suffix_no_slash ha tsuf
    have tpre : t <+: "/versions/".toList ++ (v.toList ++ ".toml".toList) := ⟨'/' :: u, h2.symm⟩
    have ht0 : t = [] := by
      by_cases hne' : t = []
      · exact hne'
      · have hmem : '/' ∈ t := by
          obtain ⟨r, hr⟩ := tpre
          have hcons : '/' :: ("versions/".toList ++ (v.toList ++ ".toml".toList)) = t ++ r :=
            hr.symm
          exact prefix_slash_mem hcons hne'
        exact absurd rfl (tnoslash '/' hmem)
    subst ht0
    rw [List.append_nil] at h1
    exact hne (packageDir_toList_inj ha hb h1)
  · -- h1 : PDb = PDa ++ t ; h2 : '/' :: u = t ++ (S ++ v.tl ++ T)
    have tsuf : t <:+ b.toList := packageDir_suffix hna hnb h1
    have tnoslash := validName_suffix_no_slash hb tsuf
    have ht0 : t = [] := by
      by_cases hne' : t = []
      · exact hne'
      · have hmem : '/' ∈ t := prefix_slash_mem h2 hne'
        exact absurd rfl (tnoslash '/' hmem)
    subst ht0
    rw [List.append_nil] at h1
    exact hne (packageDir_toList_inj ha hb h1.symm)

-- ground-truth facts used by the leanassay audit

theorem validName_hello : validName "hello" = true := by decide

theorem validName_single : validName "r" = true := by decide

theorem validName_empty : validName "" = false := by decide

theorem validName_upper : validName "Hello" = false := by decide

theorem validName_slash : validName "a/b" = false := by decide

theorem validName_toml_suffix : validName "a.toml" = false := by decide

theorem validName_dot : validName "." = false := by decide

theorem validName_dotdot : validName ".." = false := by decide

theorem validName_long :
    validName "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" = false := by
  decide

theorem validVersion_example : validVersion "2.12.2" = true := by decide

theorem validVersion_upper : validVersion "1.A" = true := by decide

theorem validVersion_tilde : validVersion "1~2" = true := by decide

theorem validVersion_empty : validVersion "" = false := by decide

theorem validVersion_toml_suffix : validVersion "1.toml" = false := by decide

theorem validVersion_slash : validVersion "a/b" = false := by decide

theorem shard_hello : shard "hello" = "he" := rfl

theorem shard_single : shard "r" = "r_" := rfl

theorem shard_two : shard "ab" = "ab" := rfl

theorem shard_empty : shard "" = "" := rfl

theorem packageDir_hello : packageDir "hello" = "packages/he/hello" := rfl

theorem packageDir_single : packageDir "r" = "packages/r_/r" := rfl

theorem versionPath_hello :
    versionPath "hello" "2.12.2" = "packages/he/hello/versions/2.12.2.toml" := rfl

end model
