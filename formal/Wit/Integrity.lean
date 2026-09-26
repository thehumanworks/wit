/-!
# A cache can deny service but cannot change content (ADR 0009)

Models what `fetch_pack_into` in `crates/wit/src/gitops/cloud.rs` accepts and
what `fill_cache_into` in `ops.rs` does with the answer.

* The client picks the commit (from `git ls-remote` against GitHub), never the
  server.
* `git index-pack --strict` names every object by the hash of its content, so
  the store it writes is `Valid`: an object is only found under its own hash.
* `git rev-list --objects --missing=error <commit>` succeeds only when the
  store is `Complete` from the commit.
* Refs, `HEAD`, and `remote.origin.url` are written by the client from its own
  inputs; the server's bytes only reach the object store.
* Any failure removes the staging directory and the caller clones from GitHub.

The hash is a parameter with collision resistance as a hypothesis
(`Injective hash`), not an axiom: the theorems say "if SHA-1 has no collision
among these objects then …".
-/

namespace Wit.Integrity

def Injective {α β : Type} (f : α → β) : Prop := ∀ a b, f a = f b → a = b

section
variable {Obj : Type} (hash : Obj → Nat) (refs : Obj → List Nat)

/-- An object store: object id to content. -/
abbrev Store (Obj : Type) := Nat → Option Obj

/-- Every object is stored under the hash of its content (`index-pack --strict`). -/
def Valid (S : Store Obj) : Prop := ∀ o c, S o = some c → hash c = o

/-- Object ids reachable from `root` through stored objects (`rev-list --objects`;
`refs` of a commit in a depth-1 clone omit the parents behind the shallow boundary). -/
inductive Reach (S : Store Obj) (root : Nat) : Nat → Prop
  | root : Reach S root root
  | step {o o' : Nat} {c : Obj} : Reach S root o → S o = some c → o' ∈ refs c → Reach S root o'

/-- `rev-list --missing=error` succeeds: every reachable id is present. -/
def Complete (S : Store Obj) (root : Nat) : Prop := ∀ o, Reach refs S root o → (S o).isSome

/-- Two valid stores complete from the same commit agree on everything reachable
from it, and reach the same ids. -/
theorem valid_complete_agree (hinj : Injective hash) {S₁ S₂ : Store Obj} {root : Nat}
    (v₁ : Valid hash S₁) (v₂ : Valid hash S₂) (c₁ : Complete refs S₁ root) (c₂ : Complete refs S₂ root) :
    ∀ o, Reach refs S₁ root o → S₁ o = S₂ o ∧ Reach refs S₂ root o := by
  intro o h
  induction h with
  | root =>
    refine ⟨?_, .root⟩
    exact agree hinj v₁ v₂ (c₁ _ .root) (c₂ _ .root)
  | @step o o' c hr hs hmem ih =>
    have r₂ : Reach refs S₂ root o' := .step ih.2 (ih.1 ▸ hs) hmem
    exact ⟨agree hinj v₁ v₂ (c₁ _ (.step hr hs hmem)) (c₂ _ r₂), r₂⟩
where
  agree {S₁ S₂ : Store Obj} {o : Nat} (hinj : Injective hash) (v₁ : Valid hash S₁) (v₂ : Valid hash S₂)
      (h₁ : (S₁ o).isSome) (h₂ : (S₂ o).isSome) : S₁ o = S₂ o := by
    cases e₁ : S₁ o with
    | none => simp [e₁] at h₁
    | some a =>
      cases e₂ : S₂ o with
      | none => simp [e₂] at h₂
      | some b =>
        have := hinj a b ((v₁ o a e₁).trans (v₂ o b e₂).symm)
        rw [this]

/-- What `wit` reads from a cache entry: the objects reachable from its commit,
and the metadata the client wrote. -/
structure Cache (Obj : Type) where
  objects : Store Obj
  commit : Nat
  branch : String
  remoteUrl : String

/-- The server's answer after transport checks (status 200, size cap, no
redirect, `x-wit-commit` match): either nothing or bytes that `index-pack`
turned into a store. -/
inductive Answer (Obj : Type) where
  | unavailable
  | pack (S : Store Obj)

/-- `fetch_pack_into`: accept a pack only if the store is valid, holds the
commit, and is complete from it; the client writes refs, HEAD and config. -/
def accepts (commit : Nat) (S : Store Obj) : Prop :=
  Valid hash S ∧ (S commit).isSome ∧ Complete refs S commit

/-- `fill_cache_into`: the cloud result when it verifies, otherwise a GitHub clone. -/
def fill (verify : Store Obj → Bool) (github : Store Obj) (url branch : String) (commit : Nat) :
    Answer Obj → Cache Obj
  | .pack S => if verify S then ⟨S, commit, branch, url⟩ else ⟨github, commit, branch, url⟩
  | .unavailable => ⟨github, commit, branch, url⟩

/-- For every answer the server can give, the filled cache has the commit the
client resolved, the client's branch and remote, and exactly GitHub's objects
on everything `wit` can reach from that commit. A server can only make `wit`
fall back to GitHub. -/
theorem fill_matches_github (hinj : Injective hash) (commit : Nat) (verify : Store Obj → Bool)
    (hverify : ∀ S, verify S = true → accepts hash refs commit S)
    {github : Store Obj} (gv : Valid hash github) (gc : Complete refs github commit)
    (url branch : String) (a : Answer Obj) :
    let C := fill verify github url branch commit a
    C.commit = commit ∧ C.branch = branch ∧ C.remoteUrl = url ∧
      ∀ o, Reach refs C.objects commit o ↔ Reach refs github commit o ∧ C.objects o = github o := by
  have same : ∀ S : Store Obj, accepts hash refs commit S →
      ∀ o, Reach refs S commit o ↔ Reach refs github commit o ∧ S o = github o := by
    intro S ⟨sv, _, sc⟩ o
    constructor
    · intro h
      have := valid_complete_agree hash refs hinj sv gv sc gc o h
      exact ⟨this.2, this.1⟩
    · intro ⟨h, _⟩
      have := valid_complete_agree hash refs hinj gv sv gc sc o h
      exact this.2
  have self : ∀ o, Reach refs github commit o ↔ Reach refs github commit o ∧ github o = github o :=
    fun o => ⟨fun h => ⟨h, rfl⟩, fun h => h.1⟩
  cases a with
  | unavailable => exact ⟨rfl, rfl, rfl, self⟩
  | pack S =>
    simp only [fill]
    split
    · rename_i hv
      exact ⟨rfl, rfl, rfl, same S (hverify S hv)⟩
    · exact ⟨rfl, rfl, rfl, self⟩

end

end Wit.Integrity
