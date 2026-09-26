import Wit.Generated.Constants

/-!
# Canonical keys (ADR 0009) and branch cache directories (ADR 0002)

**R2 keys** (`services/wit-cache/src/keys.js`). A pack lives at
`v1/github/{owner}/{repo}/{commit}.pack`, where owner and repo are lowercased
and restricted to the `COMPONENT` character class. The takedown lists
`v1/github/{owner}/{repo}/`.

* `packKey_injective`: distinct (owner, repo, commit) never share a key, so
  content keyed by SHA cannot be overwritten by another repository's pack.
* `repoPrefix_covers_iff`: the takedown prefix of `o/r` covers exactly the
  packs of `o/r`, never those of `o/r-fork` or `o-evil/r`.
* `lowercase_keeps_component`: lowercasing keeps a valid component valid.

**Branch directories** (`encode_branch_for_path` in
`crates/wit/src/gitops/ops.rs`). The disk cache stores each branch under
`branches/b-<encoded>`.

* `encodeBranch_injective_folded`: distinct branch names get distinct
  directory names even on case-insensitive filesystems (macOS, Windows), so
  `Main` and `main` never share a cache.
* `encodeBranch_path_safe`: the name has no `/`, `.`, or NUL, so it is one
  path component and never `.` or `..`.
-/

namespace Wit.Keys

/-! ## R2 pack keys -/

abbrev Str := List Char

def slash : Char := '/'

def packKey (o r c : Str) : Str :=
  Src.packKeyPrefix.toList ++ o ++ slash :: r ++ slash :: c ++ Src.packKeySuffix.toList

def repoPrefix (o r : Str) : Str :=
  Src.packKeyPrefix.toList ++ o ++ slash :: r ++ [slash]

/-- `isSafeComponent`: one to a hundred characters of `COMPONENT`. -/
def SafeComponent (s : Str) : Prop := ∀ ch ∈ s, ch ∈ Src.componentChars

theorem slash_not_component : slash ∉ Src.componentChars := by decide

theorem lowercase_keeps_component : ∀ ch ∈ Src.componentChars, ch.toLower ∈ Src.componentChars := by
  decide

theorem SafeComponent.no_slash {s : Str} (h : SafeComponent s) : slash ∉ s :=
  fun hs => slash_not_component (h slash hs)

/-- Two slash-free words followed by a slash can only be split one way. -/
theorem split_at_slash {a b x y : Str} (ha : slash ∉ a) (hb : slash ∉ b)
    (h : a ++ slash :: x = b ++ slash :: y) : a = b ∧ x = y := by
  induction a generalizing b with
  | nil =>
    cases b with
    | nil => simpa using h
    | cons c cs =>
      simp only [List.nil_append, List.cons_append, List.cons.injEq] at h
      exact absurd (h.1 ▸ List.mem_cons_self) hb
  | cons c cs ih =>
    cases b with
    | nil =>
      simp only [List.nil_append, List.cons_append, List.cons.injEq] at h
      exact absurd (h.1 ▸ List.mem_cons_self) ha
    | cons d ds =>
      simp only [List.cons_append, List.cons.injEq] at h
      have := ih (fun m => ha (List.mem_cons_of_mem _ m)) (fun m => hb (List.mem_cons_of_mem _ m)) h.2
      exact ⟨by rw [h.1, this.1], this.2⟩

theorem packKey_injective {o r c o' r' c' : Str}
    (ho : SafeComponent o) (hr : SafeComponent r) (ho' : SafeComponent o') (hr' : SafeComponent r')
    (h : packKey o r c = packKey o' r' c') : o = o' ∧ r = r' ∧ c = c' := by
  unfold packKey at h
  simp only [List.append_assoc, List.cons_append, List.append_cancel_left_eq] at h
  obtain ⟨h1, h2⟩ := split_at_slash ho.no_slash ho'.no_slash h
  obtain ⟨h3, h4⟩ := split_at_slash hr.no_slash hr'.no_slash h2
  exact ⟨h1, h3, List.append_cancel_right h4⟩

theorem repoPrefix_covers_iff {o r o' r' c : Str}
    (ho : SafeComponent o) (hr : SafeComponent r) (ho' : SafeComponent o') (hr' : SafeComponent r') :
    repoPrefix o r <+: packKey o' r' c ↔ o = o' ∧ r = r' := by
  constructor
  · rintro ⟨t, ht⟩
    unfold repoPrefix packKey at ht
    simp only [List.append_assoc, List.cons_append, List.append_cancel_left_eq] at ht
    obtain ⟨h1, h2⟩ := split_at_slash ho.no_slash ho'.no_slash ht
    obtain ⟨h3, -⟩ := split_at_slash hr.no_slash hr'.no_slash h2
    exact ⟨h1, h3⟩
  · rintro ⟨rfl, rfl⟩
    exact ⟨c ++ Src.packKeySuffix.toList, by simp [repoPrefix, packKey]⟩

/-- The R2 lifecycle rule's prefix covers every pack key, so retention applies
to every pack. -/
theorem lifecycle_covers_packs (o r c : Str) : Src.lifecycleExpirePrefix.toList <+: packKey o r c := by
  have : Src.lifecycleExpirePrefix.toList <+: Src.packKeyPrefix.toList := by decide
  obtain ⟨t, ht⟩ := this
  exact ⟨t ++ o ++ slash :: r ++ slash :: c ++ Src.packKeySuffix.toList, by
    simp only [packKey, ← ht, List.append_assoc]⟩

/-! ## Branch cache directories (ADR 0002) -/

/-- `{:02X}` digit. -/
def hexDigit (n : Nat) : Nat := if n < 10 then 48 + n else 55 + n

def percent : Nat := 37

def encodeByte (b : Nat) : List Nat :=
  if b ∈ Src.branchSafeBytes then [b] else [percent, hexDigit (b / 16), hexDigit (b % 16)]

def encodeBranch (bs : List Nat) : List Nat :=
  Src.branchDirPrefix.toList.map Char.toNat ++ bs.flatMap encodeByte

/-- ASCII case folding, as a case-insensitive filesystem compares names. -/
def fold (c : Nat) : Nat := if 65 ≤ c ∧ c ≤ 90 then c + 32 else c

def encodeByteF (b : Nat) : List Nat := (encodeByte b).map fold

theorem safe_bytes_fold_fixed : ∀ b ∈ Src.branchSafeBytes, fold b = b := by decide
theorem percent_not_safe : percent ∉ Src.branchSafeBytes := by decide
theorem fold_hex_injective : ∀ x, x < 16 → ∀ y, y < 16 → fold (hexDigit x) = fold (hexDigit y) → x = y := by
  decide

theorem encodeByteF_cancel {b b' : Nat} {xs ys : List Nat} (hb : b < 256) (hb' : b' < 256)
    (h : encodeByteF b ++ xs = encodeByteF b' ++ ys) : b = b' ∧ xs = ys := by
  unfold encodeByteF encodeByte at h
  by_cases s : b ∈ Src.branchSafeBytes <;> by_cases s' : b' ∈ Src.branchSafeBytes <;>
    simp only [s, s', ite_true, ite_false, List.map_cons, List.map_nil, List.cons_append,
      List.nil_append, List.cons.injEq] at h
  · rw [safe_bytes_fold_fixed b s, safe_bytes_fold_fixed b' s'] at h
    exact h
  · rw [safe_bytes_fold_fixed b s] at h
    have : fold percent = percent := by decide
    rw [this] at h
    exact absurd (h.1 ▸ s) percent_not_safe
  · rw [safe_bytes_fold_fixed b' s'] at h
    have : fold percent = percent := by decide
    rw [this] at h
    exact absurd (h.1.symm ▸ s') percent_not_safe
  · obtain ⟨-, hhi, hlo, rest⟩ := h
    have hi := fold_hex_injective _ (by omega) _ (by omega) hhi
    have lo := fold_hex_injective _ (by omega) _ (by omega) hlo
    exact ⟨by omega, rest⟩

theorem encodeByteF_ne_nil (b : Nat) : encodeByteF b ≠ [] := by
  unfold encodeByteF encodeByte; split <;> simp

theorem flatMap_encodeF_injective :
    ∀ (bs bs' : List Nat), (∀ b ∈ bs, b < 256) → (∀ b ∈ bs', b < 256) →
      (bs.flatMap encodeByteF = bs'.flatMap encodeByteF) → bs = bs' := by
  intro bs
  induction bs with
  | nil =>
    intro bs' _ _ h
    cases bs' with
    | nil => rfl
    | cons b' _ =>
      simp only [List.flatMap_nil, List.flatMap_cons] at h
      exact absurd (List.append_eq_nil_iff.mp h.symm).1 (encodeByteF_ne_nil b')
  | cons b rest ih =>
    intro bs' hbs hbs' h
    cases bs' with
    | nil =>
      simp only [List.flatMap_nil, List.flatMap_cons] at h
      exact absurd (List.append_eq_nil_iff.mp h).1 (encodeByteF_ne_nil b)
    | cons b' rest' =>
      simp only [List.flatMap_cons] at h
      obtain ⟨rfl, htail⟩ := encodeByteF_cancel (hbs b (by simp)) (hbs' b' (by simp)) h
      rw [ih rest' (fun x hx => hbs x (by simp [hx])) (fun x hx => hbs' x (by simp [hx])) htail]

theorem flatMap_map_fold (bs : List Nat) :
    (bs.flatMap encodeByte).map fold = bs.flatMap encodeByteF := by
  induction bs with
  | nil => rfl
  | cons b rest ih => simp [List.flatMap_cons, List.map_append, ih, encodeByteF]

/-- Distinct branch names (as UTF-8 bytes) get directory names that differ even
after ASCII case folding. -/
theorem encodeBranch_injective_folded {bs bs' : List Nat}
    (hbs : ∀ b ∈ bs, b < 256) (hbs' : ∀ b ∈ bs', b < 256)
    (h : (encodeBranch bs).map fold = (encodeBranch bs').map fold) : bs = bs' := by
  unfold encodeBranch at h
  simp only [List.map_append, flatMap_map_fold, List.append_cancel_left_eq] at h
  exact flatMap_encodeF_injective bs bs' hbs hbs' h

theorem encodeBranch_injective {bs bs' : List Nat}
    (hbs : ∀ b ∈ bs, b < 256) (hbs' : ∀ b ∈ bs', b < 256)
    (h : encodeBranch bs = encodeBranch bs') : bs = bs' :=
  encodeBranch_injective_folded hbs hbs' (by rw [h])

theorem hexDigit_safe (n : Nat) : hexDigit n ≠ 47 ∧ hexDigit n ≠ 46 ∧ hexDigit n ≠ 0 := by
  unfold hexDigit; split <;> omega

theorem safe_bytes_path_safe : ∀ c ∈ Src.branchSafeBytes, c ≠ 47 ∧ c ≠ 46 ∧ c ≠ 0 := by decide

theorem encodeByte_safe_chars (b : Nat) : ∀ c ∈ encodeByte b, c ≠ 47 ∧ c ≠ 46 ∧ c ≠ 0 := by
  unfold encodeByte
  split
  · intro c hc
    simp only [List.mem_singleton] at hc
    subst hc
    exact safe_bytes_path_safe _ ‹_›
  · intro c hc
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hc
    rcases hc with rfl | rfl | rfl
    · decide
    · exact hexDigit_safe _
    · exact hexDigit_safe _

/-- One path component: no `/`, no `.`, no NUL (so never `.` or `..`). -/
theorem encodeBranch_path_safe (bs : List Nat) :
    ∀ c ∈ encodeBranch bs, c ≠ 47 ∧ c ≠ 46 ∧ c ≠ 0 := by
  have hpre : ∀ c ∈ Src.branchDirPrefix.toList.map Char.toNat, c ≠ 47 ∧ c ≠ 46 ∧ c ≠ 0 := by decide
  intro c hc
  unfold encodeBranch at hc
  rcases List.mem_append.mp hc with hp | hb
  · exact hpre c hp
  · obtain ⟨b, _, hcb⟩ := List.mem_flatMap.mp hb
    exact encodeByte_safe_chars b c hcb

end Wit.Keys
