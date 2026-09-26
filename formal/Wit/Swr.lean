/-!
# Branch-keyed stale-while-revalidate (ADR 0002)

Models `revalidate_cache_target_with_context` in `crates/wit/src/gitops/ops.rs`
for one branch cache entry. `github s` is the content GitHub serves for commit
`s`; `fill s` is a refresh into a staging directory (cloud pack or GitHub
clone), which either fails or yields content for `s` (`Wit.Integrity` proves
this for the cloud path; the clone path is assumed, see ADR 0010).

* `revalidate_sound`: an entry always holds GitHub's content for its SHA.
* `revalidate_keeps_on_failure`: a failed `ls-remote` or refresh keeps the old
  content and SHA and records the error.
* `revalidate_recaches_only_on_change`: the entry changes only when the remote
  SHA differs from the recorded one.
* `revalidate_follows_remote`: after a successful revalidation the entry is at
  the remote SHA. Freshness depends on the SHA, never on elapsed time: time is
  not an input.
-/

namespace Wit.Swr

variable {Content : Type}

structure Entry (Content : Type) where
  sha : Nat
  content : Content
  lastError : Bool

/-- The result of `git ls-remote` for the branch. -/
inductive Remote where
  | error
  | sha (s : Nat)

def revalidate (fill : Nat → Option Content) (e : Entry Content) : Remote → Entry Content
  | .error => { e with lastError := true }
  | .sha s =>
    if s = e.sha then { e with lastError := false }
    else match fill s with
      | some c => ⟨s, c, false⟩
      | none => { e with lastError := true }

section
variable (github : Nat → Content) (fill : Nat → Option Content)

def Sound (e : Entry Content) : Prop := e.content = github e.sha

theorem revalidate_sound (hfill : ∀ s c, fill s = some c → c = github s) (e : Entry Content)
    (he : Sound github e) (r : Remote) : Sound github (revalidate fill e r) := by
  cases r with
  | error => exact he
  | sha s =>
    by_cases hs : s = e.sha
    · simpa [revalidate, hs, Sound] using he
    · cases hf : fill s with
      | none => simpa [revalidate, hs, hf, Sound] using he
      | some c => simpa [revalidate, hs, hf, Sound] using hfill s c hf

theorem revalidate_keeps_on_failure (e : Entry Content) (r : Remote)
    (hfail : r = .error ∨ ∃ s, r = .sha s ∧ s ≠ e.sha ∧ fill s = none) :
    (revalidate fill e r).sha = e.sha ∧ (revalidate fill e r).content = e.content ∧
      (revalidate fill e r).lastError = true := by
  rcases hfail with rfl | ⟨s, rfl, hs, hf⟩
  · exact ⟨rfl, rfl, rfl⟩
  · simp [revalidate, hs, hf]

theorem revalidate_recaches_only_on_change (e : Entry Content) (r : Remote)
    (h : (revalidate fill e r).sha ≠ e.sha) :
    ∃ s, r = .sha s ∧ s ≠ e.sha ∧ (revalidate fill e r).sha = s := by
  cases r with
  | error => exact absurd rfl h
  | sha s =>
    by_cases hs : s = e.sha
    · simp [revalidate, hs] at h
    · cases hf : fill s with
      | none => simp [revalidate, hs, hf] at h
      | some c => exact ⟨s, rfl, hs, by simp [revalidate, hs, hf]⟩

theorem revalidate_follows_remote (e : Entry Content) (s : Nat) (h : s = e.sha ∨ (fill s).isSome) :
    (revalidate fill e (.sha s)).sha = s ∧ (revalidate fill e (.sha s)).lastError = false := by
  by_cases hs : s = e.sha
  · simp [revalidate, hs]
  · cases hf : fill s with
    | none => simp [hf, hs] at h
    | some c => simp [revalidate, hs, hf]

end

end Wit.Swr
