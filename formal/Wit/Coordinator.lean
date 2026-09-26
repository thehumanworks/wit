import Wit.Generated.Constants

/-!
# The fill coordinator as a state machine (ADR 0009)

Models `FillCoordinator` (`services/wit-cache/src/coordinator.js`) together
with the R2 bucket it manages, the queue consumer's writes, the R2 lifecycle
rule, and the clock. `Step` lists every event; `Reachable` is any finite
sequence of events from the empty state. The theorems hold on every reachable
state, for any interleaving and any fill outcomes:

* `single_flight`: at most one pending fill per `repo@commit`.
* `inflight_le`: pending fills never exceed `MAX_INFLIGHT_FILLS`, and fills
  holding R2 bytes not yet in the ledger are among them.
* `daily_fills_le`: fills started per UTC day never exceed `DAILY_FILL_LIMIT`.
* `daily_bytes_le`: bytes counted per UTC day never exceed
  `DAILY_FILL_BYTES + MAX_INFLIGHT_FILLS × maxFillBytes` (the budget is checked
  when a fill starts; the fills already running may finish over it).
* `ledger_le_cap`: the ledger never holds more than `STORAGE_CAP_BYTES`.
* `r2_le_cap_plus_inflight`: the bytes R2 actually stores never exceed
  `STORAGE_CAP_BYTES + MAX_INFLIGHT_FILLS × MAX_PACK_BYTES`, and never exceed
  the cap when no fill is between its R2 write and its `complete`.
* `takedown_holds`: for 30 days after a takedown no fill of that repository
  starts, the ledger has no row for it, and any pack of it in R2 is an
  in-flight write whose `complete` deletes it.
* `evict_removes_oldest`: eviction removes the oldest row other than the pack
  just stored.

What the model does not cover is listed in ADR 0010.
-/

namespace Wit.Coordinator

/-- A pack identity. `Wit.Keys` shows the string key is injective in it. -/
structure Key where
  repo : Nat
  commit : Nat
  deriving DecidableEq, Repr

inductive Scope where
  | repo (r : Nat)
  | key (k : Key)
  | global
  deriving DecidableEq

/-- A ledger row (`packs` table). -/
structure Row where
  key : Key
  bytes : Nat
  filledAt : Nat

/-- An R2 object. -/
structure Obj where
  key : Key
  bytes : Nat
  createdAt : Nat

def day : Nat := 86400

def dayOf (t : Nat) : Nat := t / day

structure Params where
  lim : Limits
  pendingTtl : Nat
  graceDays : Nat
  lifecycleDays : Nat
  lifecycleDelayDays : Nat
  /-- Most bytes one fill can report: the pack cap plus one side-band chunk. -/
  maxFillBytes : Nat
  ttl : Reason → Nat
  repoScoped : Reason → Bool
  rateCap : Nat

/-- The side conditions the proofs need, checked on the real values in `Wit.Budget`. -/
structure Params.WF (P : Params) : Prop where
  pack_le_cap : P.lim.maxPackBytes ≤ P.lim.storageCapBytes
  pack_le_fill : P.lim.maxPackBytes ≤ P.maxFillBytes
  ledger_outlives : P.lifecycleDays + P.lifecycleDelayDays ≤ P.lim.retentionDays + P.graceDays
  lifecycle_pos : 0 < P.lifecycleDays + P.lifecycleDelayDays
  block_dominates : ∀ r, P.repoScoped r = true → r ≠ .blocked → P.pendingTtl + P.ttl r ≤ P.ttl .blocked

structure State where
  clock : Nat
  pending : List (Key × Nat)
  fills : Nat → Nat
  bytes : Nat → Nat
  negatives : Scope → Option (Reason × Nat)
  ledger : List Row
  r2 : List Obj
  /-- Ghost: fills whose pack is in R2 but whose `complete` has not run yet. -/
  writing : List Key

def State.init : State := ⟨0, [], fun _ => 0, fun _ => 0, fun _ => none, [], [], []⟩

def pendingKeys (s : State) : List Key := s.pending.map Prod.fst

def sumBy {α : Type} (f : α → Nat) : List α → Nat
  | [] => 0
  | x :: xs => f x + sumBy f xs

def live (s : State) (sc : Scope) : Bool :=
  match s.negatives sc with
  | some (_, u) => decide (s.clock < u)
  | none => false

/-- `isBlocked`: a live `blocked` negative on the repository. -/
def blocked (s : State) (r : Nat) : Bool :=
  match s.negatives (.repo r) with
  | some (.blocked, u) => decide (s.clock < u)
  | _ => false

def bump (f : Nat → Nat) (d n : Nat) : Nat → Nat := fun d' => if d' = d then f d' + n else f d'

/-- `bumpDaily(day, -1, 0)` with `MAX(0, fills - 1)`. -/
def dec (f : Nat → Nat) (d : Nat) : Nat → Nat := fun d' => if d' = d then f d' - 1 else f d'

def prune (P : Params) (s : State) : State :=
  { s with
    negatives := fun sc =>
      match s.negatives sc with
      | some (r, u) => if u ≤ s.clock then none else some (r, u)
      | none => none
    pending := s.pending.filter (fun p => decide (s.clock < p.2 + P.pendingTtl))
    ledger := s.ledger.filter
      (fun row => decide (s.clock < row.filledAt + (P.lim.retentionDays + P.graceDays) * day)) }

inductive Decision where
  | skipped | pending | busy | budget | queued
  deriving DecidableEq

def requestFill (P : Params) (s₀ : State) (k : Key) : Decision × State :=
  let s := prune P s₀
  let d := dayOf s.clock
  if live s (.repo k.repo) || live s (.key k) || live s .global then (.skipped, s)
  else if k ∈ pendingKeys s then (.pending, s)
  else if P.lim.maxInflightFills ≤ s.pending.length then (.busy, s)
  else if P.lim.dailyFillLimit ≤ s.fills d ∨ P.lim.dailyFillBytes ≤ s.bytes d then (.budget, s)
  else (.queued, { s with pending := s.pending ++ [(k, s.clock)], fills := bump s.fills d 1 })

def release (s : State) (k : Key) : State :=
  { s with pending := s.pending.filter (fun p => decide (p.1 ≠ k)), fills := dec s.fills (dayOf s.clock) }

/-- The consumer's R2 write (single PUT or completed multipart upload). -/
def store (s : State) (k : Key) (b : Nat) : State :=
  { s with r2 := s.r2.filter (fun o => decide (o.key ≠ k)) ++ [⟨k, b, s.clock⟩], writing := k :: s.writing }

/-- Negative upsert keeping the later expiry. -/
def upsertNeg (neg : Scope → Option (Reason × Nat)) (sc : Scope) (r : Reason) (u : Nat) :
    Scope → Option (Reason × Nat) := fun sc' =>
  if sc' = sc then
    match neg sc with
    | some (r0, u0) => if u0 < u then some (r, u) else some (r0, u0)
    | none => some (r, u)
  else neg sc'

def ttlFor (P : Params) (r : Reason) (retry : Nat) : Nat :=
  if r = .rateLimited ∧ 0 < retry then min retry P.rateCap else P.ttl r

def scopeFor (P : Params) (k : Key) (r : Reason) : Scope :=
  if r = .rateLimited then .global else if P.repoScoped r then .repo k.repo else .key k

def oldestExcept (keep : Key) : List Row → Option Row
  | [] => none
  | r :: rs =>
    if r.key = keep then oldestExcept keep rs
    else match oldestExcept keep rs with
      | none => some r
      | some a => if a.filledAt < r.filledAt then some a else some r

def evict (cap : Nat) (keep : Key) : Nat → List Row → List Obj → List Row × List Obj
  | 0, rows, objs => (rows, objs)
  | n + 1, rows, objs =>
    if sumBy Row.bytes rows ≤ cap then (rows, objs)
    else match oldestExcept keep rows with
      | none => (rows, objs)
      | some o => evict cap keep n (rows.filter (fun r => decide (r.key ≠ o.key)))
          (objs.filter (fun x => decide (x.key ≠ o.key)))

def upsertRow (rows : List Row) (row : Row) : List Row :=
  rows.filter (fun r => decide (r.key ≠ row.key)) ++ [row]

inductive Outcome where
  | ok (bytes : Nat) (reused : Bool)
  | fail (reason : Reason) (retryAfter : Nat) (bytesRead : Nat)

def complete (P : Params) (s : State) (k : Key) : Outcome → State
  | .fail r retry n =>
    { s with
      pending := s.pending.filter (fun p => decide (p.1 ≠ k))
      negatives := upsertNeg s.negatives (scopeFor P k r) r (s.clock + ttlFor P r retry)
      bytes := bump s.bytes (dayOf s.clock) n }
  | .ok b reused =>
    let base := { s with
      pending := s.pending.filter (fun p => decide (p.1 ≠ k))
      writing := s.writing.erase k
      bytes := bump s.bytes (dayOf s.clock) (if reused then 0 else b) }
    if blocked s k.repo then
      { base with
        r2 := s.r2.filter (fun o => decide (o.key ≠ k))
        ledger := s.ledger.filter (fun r => decide (r.key ≠ k)) }
    else
      let res := evict P.lim.storageCapBytes k (s.ledger.length + 1) (upsertRow s.ledger ⟨k, b, s.clock⟩) s.r2
      { base with ledger := res.1, r2 := res.2 }

def takedown (P : Params) (s : State) (r : Nat) : State :=
  { s with
    r2 := s.r2.filter (fun o => decide (o.key.repo ≠ r))
    ledger := s.ledger.filter (fun row => decide (row.key.repo ≠ r))
    negatives := fun sc => if sc = .repo r then some (.blocked, s.clock + P.ttl .blocked) else s.negatives sc }

/-- Advance the clock. R2's lifecycle rule has removed every object older than
`lifecycleDays + lifecycleDelayDays` by then (the delay is an assumption). -/
def tick (P : Params) (s : State) (t : Nat) : State :=
  { s with
    clock := t
    r2 := s.r2.filter
      (fun o => decide (t < o.createdAt + (P.lifecycleDays + P.lifecycleDelayDays) * day)) }

def pendingLive (P : Params) (s : State) (t : Nat) (k : Key) : Prop :=
  ∀ p ∈ s.pending, p.1 = k → t < p.2 + P.pendingTtl

inductive Step (P : Params) : State → State → Prop
  | request (s : State) (k : Key) : Step P s (requestFill P s k).2
  | release (s : State) (k : Key) (hw : k ∉ s.writing) : Step P s (release s k)
  | store (s : State) (k : Key) (b : Nat) (hk : k ∈ pendingKeys s) (hw : k ∉ s.writing)
      (hb : b ≤ P.lim.maxPackBytes) (hlive : pendingLive P s s.clock k) : Step P s (store s k b)
  | completeOk (s : State) (k : Key) (b : Nat) (reused : Bool) (hk : k ∈ pendingKeys s)
      (hb : b ≤ P.lim.maxPackBytes) (hsize : ∀ o ∈ s.r2, o.key = k → o.bytes ≤ b)
      (hlive : pendingLive P s s.clock k) : Step P s (complete P s k (.ok b reused))
  | completeFail (s : State) (k : Key) (r : Reason) (retry n : Nat) (hk : k ∈ pendingKeys s)
      (hw : k ∉ s.writing) (hn : n ≤ P.maxFillBytes) (hlive : pendingLive P s s.clock k) :
      Step P s (complete P s k (.fail r retry n))
  | prune (s : State) : Step P s (prune P s)
  | takedown (s : State) (r : Nat) : Step P s (takedown P s r)
  | expire (s : State) (k : Key) : Step P s { s with r2 := s.r2.filter (fun o => decide (o.key ≠ k)) }
  | tick (s : State) (t : Nat) (ht : s.clock ≤ t) (hw : ∀ k ∈ s.writing, pendingLive P s t k) :
      Step P s (tick P s t)

inductive Reachable (P : Params) : State → Prop
  | init : Reachable P State.init
  | step {s s' : State} : Reachable P s → Step P s s' → Reachable P s'

/-! ## List lemmas -/

@[simp] theorem sumBy_nil {α : Type} (f : α → Nat) : sumBy f [] = 0 := rfl
@[simp] theorem sumBy_cons {α : Type} (f : α → Nat) (x : α) (xs : List α) :
    sumBy f (x :: xs) = f x + sumBy f xs := rfl

@[simp] theorem filter_const_true {α : Type} (l : List α) : l.filter (fun _ => true) = l :=
  List.filter_eq_self.mpr (by simp)

theorem sumBy_append {α : Type} (f : α → Nat) (l₁ l₂ : List α) :
    sumBy f (l₁ ++ l₂) = sumBy f l₁ + sumBy f l₂ := by
  induction l₁ with
  | nil => simp
  | cons x xs ih => simp [ih]; omega

theorem sumBy_split {α : Type} (f : α → Nat) (p : α → Bool) (l : List α) :
    sumBy f l = sumBy f (l.filter p) + sumBy f (l.filter (fun x => !p x)) := by
  induction l with
  | nil => simp
  | cons x xs ih => cases h : p x <;> simp [h, ih] <;> omega

theorem sumBy_filter_le {α : Type} (f : α → Nat) (p : α → Bool) (l : List α) :
    sumBy f (l.filter p) ≤ sumBy f l := by
  have := sumBy_split f p l; omega

theorem le_sumBy_of_mem {α : Type} (f : α → Nat) {x : α} {l : List α} (h : x ∈ l) : f x ≤ sumBy f l := by
  induction l with
  | nil => cases h
  | cons y ys ih =>
    rcases List.mem_cons.mp h with rfl | h
    · simp
    · have := ih h; simp; omega

theorem sumBy_le_mul {α : Type} (f : α → Nat) (m : Nat) (l : List α) (h : ∀ x ∈ l, f x ≤ m) :
    sumBy f l ≤ l.length * m := by
  induction l with
  | nil => simp
  | cons x xs ih =>
    simp only [sumBy_cons, List.length_cons, Nat.succ_mul]
    have := h x (by simp)
    have := ih (fun y hy => h y (by simp [hy]))
    omega

theorem nodup_map_filter {α β : Type} (f : α → β) (p : α → Bool) {l : List α} (h : (l.map f).Nodup) :
    ((l.filter p).map f).Nodup :=
  h.sublist (List.filter_sublist.map f)

/-- Removing the pending row of a key that is pending shortens the list by one. -/
theorem length_filter_key {k : Key} :
    ∀ {l : List (Key × Nat)}, (l.map Prod.fst).Nodup → k ∈ l.map Prod.fst →
      (l.filter (fun p => decide (p.1 ≠ k))).length + 1 = l.length
  | [], _, h => by simp at h
  | p :: ps, hn, hk => by
    rw [List.map_cons, List.nodup_cons] at hn
    by_cases hp : p.1 = k
    · have : ps.filter (fun p => decide (p.1 ≠ k)) = ps := by
        apply List.filter_eq_self.mpr
        intro q hq
        simp only [ne_eq, decide_eq_true_eq]
        intro hqk
        exact hn.1 (hp ▸ hqk ▸ List.mem_map_of_mem hq)
      rw [List.filter_cons_of_neg (by simpa using hp), this]
      simp
    · have hk' : k ∈ ps.map Prod.fst := by
        rcases List.mem_cons.mp (List.map_cons ▸ hk) with h | h
        · exact absurd h.symm hp
        · exact h
      have := length_filter_key hn.2 hk'
      rw [List.filter_cons_of_pos (by simpa using hp)]
      simp only [List.length_cons]
      omega

theorem dominated_sum {xs : List Obj} {rows : List Row} (hx : (xs.map Obj.key).Nodup)
    (h : ∀ o ∈ xs, ∃ row ∈ rows, row.key = o.key ∧ o.bytes ≤ row.bytes) :
    sumBy Obj.bytes xs ≤ sumBy Row.bytes rows := by
  induction xs generalizing rows with
  | nil => simp
  | cons o rest ih =>
    rw [List.map_cons, List.nodup_cons] at hx
    obtain ⟨row, hrow, hkey, hle⟩ := h o (by simp)
    have hrest : sumBy Obj.bytes rest ≤ sumBy Row.bytes (rows.filter (fun r => decide (r.key ≠ o.key))) := by
      apply ih hx.2
      intro o' ho'
      obtain ⟨r', hr', hk', hle'⟩ := h o' (by simp [ho'])
      refine ⟨r', List.mem_filter.mpr ⟨hr', ?_⟩, hk', hle'⟩
      simp only [ne_eq, decide_eq_true_eq, hk']
      intro heq
      exact hx.1 (heq ▸ List.mem_map_of_mem ho')
    have hsplit := sumBy_split Row.bytes (fun r => decide (r.key ≠ o.key)) rows
    have hin : row ∈ rows.filter (fun r => !decide (r.key ≠ o.key)) := by
      simp [List.mem_filter, hrow, hkey]
    have := le_sumBy_of_mem Row.bytes hin
    simp only [sumBy_cons]
    omega

/-! ## Eviction -/

theorem oldestExcept_mem {keep : Key} :
    ∀ {rows : List Row} {o : Row}, oldestExcept keep rows = some o → o ∈ rows ∧ o.key ≠ keep
  | [], _, h => by simp [oldestExcept] at h
  | r :: rs, o, h => by
    unfold oldestExcept at h
    by_cases hr : r.key = keep
    · simp only [hr, ite_true] at h
      have := oldestExcept_mem h
      exact ⟨List.mem_cons_of_mem _ this.1, this.2⟩
    · simp only [hr, ite_false] at h
      split at h
      · cases h; exact ⟨by simp, hr⟩
      · rename_i a ha
        split at h
        · cases h; have := oldestExcept_mem ha; exact ⟨List.mem_cons_of_mem _ this.1, this.2⟩
        · cases h; exact ⟨by simp, hr⟩

theorem oldestExcept_none {keep : Key} :
    ∀ {rows : List Row}, oldestExcept keep rows = none → ∀ r ∈ rows, r.key = keep
  | [], _, _, h => by cases h
  | r :: rs, h, x, hx => by
    unfold oldestExcept at h
    by_cases hr : r.key = keep
    · simp only [hr, ite_true] at h
      rcases List.mem_cons.mp hx with rfl | hx
      · exact hr
      · exact oldestExcept_none h x hx
    · simp only [hr, ite_false] at h
      split at h
      · cases h
      · split at h <;> cases h

theorem oldestExcept_min {keep : Key} :
    ∀ {rows : List Row} {o : Row}, oldestExcept keep rows = some o →
      ∀ r ∈ rows, r.key ≠ keep → o.filledAt ≤ r.filledAt
  | [], _, h => by simp [oldestExcept] at h
  | r :: rs, o, h => by
    intro x hx hxk
    unfold oldestExcept at h
    by_cases hr : r.key = keep
    · simp only [hr, ite_true] at h
      rcases List.mem_cons.mp hx with rfl | hx
      · exact absurd hr hxk
      · exact oldestExcept_min h x hx hxk
    · simp only [hr, ite_false] at h
      split at h
      · rename_i hnone
        cases h
        rcases List.mem_cons.mp hx with rfl | hx
        · exact Nat.le_refl _
        · exact absurd (oldestExcept_none hnone x hx) hxk
      · rename_i a ha
        have hmin := oldestExcept_min ha
        split at h
        · rename_i hlt
          cases h
          rcases List.mem_cons.mp hx with rfl | hx
          · omega
          · exact hmin x hx hxk
        · rename_i hge
          cases h
          rcases List.mem_cons.mp hx with rfl | hx
          · exact Nat.le_refl _
          · have := hmin x hx hxk; omega

/-- Eviction removes the rows of a set of keys (never `keep`) and exactly the R2
objects with those keys, and leaves at most `cap` bytes in the ledger. -/
theorem evict_spec (cap : Nat) (keep : Key) :
    ∀ (n : Nat) (rows : List Row) (objs : List Obj), rows.length ≤ n → (rows.map Row.key).Nodup →
      (∀ r ∈ rows, r.key = keep → r.bytes ≤ cap) →
      ∃ K : List Key, keep ∉ K ∧
        (evict cap keep n rows objs).1 = rows.filter (fun r => decide (r.key ∉ K)) ∧
        (evict cap keep n rows objs).2 = objs.filter (fun o => decide (o.key ∉ K)) ∧
        sumBy Row.bytes (evict cap keep n rows objs).1 ≤ cap
  | 0, rows, objs, hlen, _, _ => by
    have : rows = [] := List.eq_nil_of_length_eq_zero (by omega)
    subst this
    exact ⟨[], by simp, by simp [evict], by simp [evict], by simp [evict]⟩
  | n + 1, rows, objs, hlen, hnd, hkeep => by
    unfold evict
    by_cases hle : sumBy Row.bytes rows ≤ cap
    · rw [ite_eq_left hle]
      exact ⟨[], by simp, by simp, by simp, hle⟩
    · rw [ite_eq_right hle]
      split
      · rename_i hnone
        refine ⟨[], by simp, by simp, by simp, ?_⟩
        have hall := oldestExcept_none hnone
        match rows, hnd with
        | [], _ => simp
        | [r], _ => simpa using hkeep r (by simp) (hall r (by simp))
        | a :: b :: _, hnd =>
          rw [List.map_cons, List.map_cons, List.nodup_cons] at hnd
          exact absurd (by rw [hall a (by simp), hall b (by simp)] : a.key = b.key)
            (fun h => hnd.1 (h ▸ List.mem_cons_self))
      · rename_i o ho
        obtain ⟨hmem, hne⟩ := oldestExcept_mem ho
        have hshort : (rows.filter (fun r => decide (r.key ≠ o.key))).length ≤ n := by
          have : (rows.filter (fun r => decide (r.key ≠ o.key))).length < rows.length :=
            List.length_filter_lt_length_iff_exists.mpr ⟨o, hmem, by simp⟩
          omega
        obtain ⟨K, hK, h1, h2, h3⟩ := evict_spec cap keep n _ (objs.filter (fun x => decide (x.key ≠ o.key)))
          hshort (nodup_map_filter _ _ hnd) (fun r hr hk => hkeep r (List.mem_filter.mp hr).1 hk)
        refine ⟨o.key :: K, ?_, ?_, ?_, h3⟩
        · simp only [List.mem_cons, not_or]; exact ⟨fun h => hne h.symm, hK⟩
        · rw [h1, List.filter_filter]
          apply List.filter_congr
          intro x _
          by_cases e1 : x.key = o.key <;> by_cases e2 : x.key ∈ K <;> simp [e1, e2]
        · rw [h2, List.filter_filter]
          apply List.filter_congr
          intro x _
          by_cases e1 : x.key = o.key <;> by_cases e2 : x.key ∈ K <;> simp [e1, e2]

theorem evict_sub (cap : Nat) (keep : Key) : ∀ (n : Nat) (rows : List Row) (objs : List Obj),
    (∀ x ∈ (evict cap keep n rows objs).1, x ∈ rows) ∧ (∀ o ∈ (evict cap keep n rows objs).2, o ∈ objs)
  | 0, rows, objs => ⟨fun _ h => h, fun _ h => h⟩
  | n + 1, rows, objs => by
    unfold evict
    split
    · exact ⟨fun _ h => h, fun _ h => h⟩
    · split
      · exact ⟨fun _ h => h, fun _ h => h⟩
      · rename_i o _
        have ih := evict_sub cap keep n (rows.filter (fun r => decide (r.key ≠ o.key)))
          (objs.filter (fun x => decide (x.key ≠ o.key)))
        exact ⟨fun x h => (List.mem_filter.mp (ih.1 x h)).1, fun y h => (List.mem_filter.mp (ih.2 y h)).1⟩

/-- The row eviction removes first is no younger than any other candidate. -/
theorem evict_removes_oldest {keep : Key} {rows : List Row} {o : Row}
    (h : oldestExcept keep rows = some o) : o ∈ rows ∧ o.key ≠ keep ∧
      ∀ r ∈ rows, r.key ≠ keep → o.filledAt ≤ r.filledAt :=
  ⟨(oldestExcept_mem h).1, (oldestExcept_mem h).2, oldestExcept_min h⟩

/-! ## The invariant -/

structure Inv (P : Params) (s : State) : Prop where
  pendingNodup : (pendingKeys s).Nodup
  pendingLe : s.pending.length ≤ P.lim.maxInflightFills
  pendingClock : ∀ p ∈ s.pending, p.2 ≤ s.clock
  fillsLe : ∀ d, s.fills d ≤ P.lim.dailyFillLimit
  bytesToday : s.bytes (dayOf s.clock) + P.maxFillBytes * s.pending.length ≤
    P.lim.dailyFillBytes + P.maxFillBytes * P.lim.maxInflightFills
  bytesLe : ∀ d, s.bytes d ≤ P.lim.dailyFillBytes + P.maxFillBytes * P.lim.maxInflightFills
  bytesFuture : ∀ d, dayOf s.clock < d → s.bytes d = 0
  ledgerNodup : (s.ledger.map Row.key).Nodup
  ledgerCap : sumBy Row.bytes s.ledger ≤ P.lim.storageCapBytes
  r2Nodup : (s.r2.map Obj.key).Nodup
  r2Le : ∀ o ∈ s.r2, o.bytes ≤ P.lim.maxPackBytes
  r2Clock : ∀ o ∈ s.r2, o.createdAt ≤ s.clock
  r2Fresh : ∀ o ∈ s.r2, s.clock < o.createdAt + (P.lifecycleDays + P.lifecycleDelayDays) * day
  tracked : ∀ o ∈ s.r2, o.key ∈ s.writing ∨
    ∃ row ∈ s.ledger, row.key = o.key ∧ o.bytes ≤ row.bytes ∧ o.createdAt ≤ row.filledAt
  writingNodup : s.writing.Nodup
  writingPending : ∀ k ∈ s.writing, k ∈ pendingKeys s
  writingLive : ∀ k ∈ s.writing, pendingLive P s s.clock k

theorem inv_init (P : Params) : Inv P State.init where
  pendingNodup := by simp [pendingKeys, State.init]
  pendingLe := by simp [State.init]
  pendingClock := by simp [State.init]
  fillsLe := by simp [State.init]
  bytesToday := by simp [State.init]
  bytesLe := by simp [State.init]
  bytesFuture := by simp [State.init]
  ledgerNodup := by simp [State.init]
  ledgerCap := by simp [State.init]
  r2Nodup := by simp [State.init]
  r2Le := by simp [State.init]
  r2Clock := by simp [State.init]
  r2Fresh := by simp [State.init]
  tracked := by simp [State.init]
  writingNodup := by simp [State.init]
  writingPending := by simp [State.init]
  writingLive := by simp [State.init]

theorem mem_pendingKeys_filter {s : List (Key × Nat)} {q : Key × Nat → Bool} {k : Key} :
    k ∈ (s.filter q).map Prod.fst → k ∈ s.map Prod.fst := by
  intro h
  obtain ⟨p, hp, rfl⟩ := List.mem_map.mp h
  exact List.mem_map_of_mem (List.mem_filter.mp hp).1

theorem inv_prune {P : Params} (W : P.WF) {s : State} (I : Inv P s) : Inv P (prune P s) := by
  have hlen : (s.pending.filter (fun p => decide (s.clock < p.2 + P.pendingTtl))).length ≤ s.pending.length :=
    List.length_filter_le _ _
  refine ⟨nodup_map_filter _ _ I.pendingNodup, Nat.le_trans hlen I.pendingLe,
    fun p hp => I.pendingClock p (List.mem_filter.mp hp).1, I.fillsLe, ?_, I.bytesLe, I.bytesFuture,
    nodup_map_filter _ _ I.ledgerNodup,
    Nat.le_trans (sumBy_filter_le _ _ _) I.ledgerCap, I.r2Nodup, I.r2Le, I.r2Clock, I.r2Fresh, ?_,
    I.writingNodup, ?_, ?_⟩
  · have := Nat.mul_le_mul_left P.maxFillBytes hlen
    have := I.bytesToday
    simp only [prune]; omega
  · intro o ho
    rcases I.tracked o ho with hw | ⟨row, hrow, hk, hb, hc⟩
    · exact Or.inl hw
    · refine Or.inr ⟨row, List.mem_filter.mpr ⟨hrow, ?_⟩, hk, hb, hc⟩
      simp only [decide_eq_true_eq]
      have hf := I.r2Fresh o ho
      have := Nat.mul_le_mul_right day W.ledger_outlives
      omega
  · intro k hk
    obtain ⟨p, hp, hpk⟩ := List.mem_map.mp (I.writingPending k hk)
    exact List.mem_map.mpr ⟨p, List.mem_filter.mpr ⟨hp, by
      simp only [decide_eq_true_eq]; exact I.writingLive k hk p hp hpk⟩, hpk⟩
  · intro k hk p hp hpk
    exact I.writingLive k hk p (List.mem_filter.mp hp).1 hpk

theorem requestFill_cases (P : Params) (s : State) (k : Key) :
    ((requestFill P s k).1 ≠ .queued ∧ (requestFill P s k).2 = prune P s) ∨
      ((requestFill P s k).1 = .queued ∧
        k ∉ pendingKeys (prune P s) ∧
        (prune P s).pending.length < P.lim.maxInflightFills ∧
        (prune P s).fills (dayOf s.clock) < P.lim.dailyFillLimit ∧
        (prune P s).bytes (dayOf s.clock) < P.lim.dailyFillBytes ∧
        (live (prune P s) (.repo k.repo) || live (prune P s) (.key k) || live (prune P s) .global) = false ∧
        (requestFill P s k).2 = { prune P s with
          pending := (prune P s).pending ++ [(k, s.clock)]
          fills := bump (prune P s).fills (dayOf s.clock) 1 }) := by
  unfold requestFill
  simp only [show (prune P s).clock = s.clock from rfl]
  split
  · exact Or.inl ⟨by simp, rfl⟩
  · rename_i hlive
    split
    · exact Or.inl ⟨by simp, rfl⟩
    · rename_i hpend
      split
      · exact Or.inl ⟨by simp, rfl⟩
      · rename_i hbusy
        split
        · exact Or.inl ⟨by simp, rfl⟩
        · rename_i hbudget
          simp only [Bool.not_eq_true] at hlive
          simp only [not_or, Nat.not_le] at hbudget
          exact Or.inr ⟨rfl, hpend, by omega, hbudget.1, hbudget.2, hlive, rfl⟩

theorem inv_request {P : Params} (W : P.WF) {s : State} (I : Inv P s) (k : Key) :
    Inv P (requestFill P s k).2 := by
  have I' := inv_prune W I
  rcases requestFill_cases P s k with ⟨_, h⟩ | ⟨_, hk, hlen, hfills, hbytes, _, h⟩
  · rw [h]; exact I'
  · rw [h]
    have hc : (prune P s).clock = s.clock := rfl
    refine ⟨?_, ?_, ?_, ?_, ?_, ?_, I'.bytesFuture, I'.ledgerNodup, I'.ledgerCap, I'.r2Nodup, I'.r2Le,
      I'.r2Clock, I'.r2Fresh, I'.tracked, I'.writingNodup, ?_, ?_⟩
    · simp only [pendingKeys, List.map_append, List.map_cons, List.map_nil]
      exact List.nodup_append.mpr ⟨I'.pendingNodup, by simp, by
        intro a ha b hb; simp at hb; subst hb; intro h; exact hk (h ▸ ha)⟩
    · simp; omega
    · intro p hp
      rcases List.mem_append.mp hp with hp | hp
      · exact I'.pendingClock p hp
      · simp at hp; subst hp; exact Nat.le_refl _
    · intro d; simp only [bump]; split
      · subst_vars; omega
      · exact I'.fillsLe d
    · simp only [List.length_append, List.length_singleton, hc]
      have := Nat.mul_le_mul_left P.maxFillBytes (show (prune P s).pending.length + 1 ≤ P.lim.maxInflightFills by omega)
      simp only [Nat.mul_add, Nat.mul_one] at this ⊢
      omega
    · intro d
      exact I'.bytesLe d
    · intro w hw
      show w ∈ ((prune P s).pending ++ _).map Prod.fst
      rw [List.map_append]
      exact List.mem_append_left _ (I'.writingPending w hw)
    · intro w hw p hp hpw
      rcases List.mem_append.mp hp with hp | hp
      · exact I'.writingLive w hw p hp hpw
      · simp at hp; subst hp
        simp at hpw; subst hpw
        exact absurd (I'.writingPending _ hw) hk

theorem inv_release {P : Params} {s : State} (I : Inv P s) (k : Key) (hw : k ∉ s.writing) :
    Inv P (release s k) := by
  have hlen : (s.pending.filter (fun p => decide (p.1 ≠ k))).length ≤ s.pending.length :=
    List.length_filter_le _ _
  refine ⟨nodup_map_filter _ _ I.pendingNodup, Nat.le_trans hlen I.pendingLe,
    fun p hp => I.pendingClock p (List.mem_filter.mp hp).1, ?_, ?_, I.bytesLe, I.bytesFuture,
    I.ledgerNodup, I.ledgerCap, I.r2Nodup, I.r2Le, I.r2Clock, I.r2Fresh, I.tracked, I.writingNodup, ?_, ?_⟩
  · intro d; simp only [release, dec]; split
    · have := I.fillsLe d; omega
    · exact I.fillsLe d
  · have := Nat.mul_le_mul_left P.maxFillBytes hlen
    have := I.bytesToday
    simp only [release]; omega
  · intro w hww
    obtain ⟨p, hp, hpw⟩ := List.mem_map.mp (I.writingPending w hww)
    refine List.mem_map.mpr ⟨p, List.mem_filter.mpr ⟨hp, ?_⟩, hpw⟩
    simp only [ne_eq, decide_eq_true_eq, hpw]
    intro h; exact hw (h ▸ hww)
  · intro w hww p hp hpw
    exact I.writingLive w hww p (List.mem_filter.mp hp).1 hpw

theorem inv_store {P : Params} (W : P.WF) {s : State} (I : Inv P s) (k : Key) (b : Nat)
    (hk : k ∈ pendingKeys s) (hw : k ∉ s.writing) (hb : b ≤ P.lim.maxPackBytes)
    (hlive : pendingLive P s s.clock k) : Inv P (store s k b) := by
  have hfilt : ∀ o ∈ s.r2.filter (fun o => decide (o.key ≠ k)), o ∈ s.r2 ∧ o.key ≠ k := by
    intro o ho; have := List.mem_filter.mp ho; simpa using this
  refine ⟨I.pendingNodup, I.pendingLe, I.pendingClock, I.fillsLe, I.bytesToday, I.bytesLe, I.bytesFuture,
    I.ledgerNodup, I.ledgerCap, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · simp only [store, List.map_append, List.map_cons, List.map_nil]
    refine List.nodup_append.mpr ⟨nodup_map_filter _ _ I.r2Nodup, by simp, ?_⟩
    intro a ha c hc
    simp at hc; subst hc
    obtain ⟨o, ho, rfl⟩ := List.mem_map.mp ha
    exact (hfilt o ho).2
  · intro o ho
    rcases List.mem_append.mp ho with ho | ho
    · exact I.r2Le o (hfilt o ho).1
    · simp at ho; subst ho; exact hb
  · intro o ho
    rcases List.mem_append.mp ho with ho | ho
    · exact I.r2Clock o (hfilt o ho).1
    · simp at ho; subst ho; exact Nat.le_refl _
  · intro o ho
    rcases List.mem_append.mp ho with ho | ho
    · exact I.r2Fresh o (hfilt o ho).1
    · simp at ho; subst ho
      have := Nat.mul_le_mul_right day W.lifecycle_pos
      simp only [store, day] at this ⊢; omega
  · intro o ho
    rcases List.mem_append.mp ho with ho | ho
    · rcases I.tracked o (hfilt o ho).1 with h | h
      · exact Or.inl (List.mem_cons_of_mem _ h)
      · exact Or.inr h
    · simp at ho; subst ho; exact Or.inl List.mem_cons_self
  · exact List.nodup_cons.mpr ⟨hw, I.writingNodup⟩
  · intro w hww
    rcases List.mem_cons.mp hww with rfl | hww
    · exact hk
    · exact I.writingPending w hww
  · intro w hww
    rcases List.mem_cons.mp hww with rfl | hww
    · exact hlive
    · exact I.writingLive w hww

/-- Facts shared by both `complete` outcomes about the pending list. -/
theorem complete_pending {P : Params} {s : State} (I : Inv P s) {k : Key} (hk : k ∈ pendingKeys s) :
    (s.pending.filter (fun p => decide (p.1 ≠ k))).length + 1 = s.pending.length :=
  length_filter_key I.pendingNodup hk

theorem bytes_after_complete {P : Params} {s : State} (I : Inv P s) {k : Key} (hk : k ∈ pendingKeys s)
    (n : Nat) (hn : n ≤ P.maxFillBytes) :
    bump s.bytes (dayOf s.clock) n (dayOf s.clock) +
        P.maxFillBytes * (s.pending.filter (fun p => decide (p.1 ≠ k))).length ≤
      P.lim.dailyFillBytes + P.maxFillBytes * P.lim.maxInflightFills := by
  have hlen := complete_pending I hk
  have := I.bytesToday
  rw [← hlen, Nat.mul_succ] at this
  simp only [bump, ite_true]
  omega

theorem bytesLe_after_complete {P : Params} {s : State} (I : Inv P s) {k : Key} (hk : k ∈ pendingKeys s)
    (n : Nat) (hn : n ≤ P.maxFillBytes) :
    ∀ d, bump s.bytes (dayOf s.clock) n d ≤ P.lim.dailyFillBytes + P.maxFillBytes * P.lim.maxInflightFills := by
  intro d
  by_cases hd : d = dayOf s.clock
  · subst hd; have := bytes_after_complete I hk n hn; omega
  · simp only [bump, hd, ite_false]; exact I.bytesLe d

theorem bytesFuture_after_complete {P : Params} {s : State} (I : Inv P s) (n : Nat) :
    ∀ d, dayOf s.clock < d → bump s.bytes (dayOf s.clock) n d = 0 := by
  intro d hd
  simp only [bump, show d ≠ dayOf s.clock by omega, ite_false]
  exact I.bytesFuture d hd

theorem inv_completeFail {P : Params} {s : State} (I : Inv P s) (k : Key) (r : Reason) (retry n : Nat)
    (hk : k ∈ pendingKeys s) (hw : k ∉ s.writing) (hn : n ≤ P.maxFillBytes) :
    Inv P (complete P s k (.fail r retry n)) := by
  have hlen := complete_pending I hk
  refine ⟨nodup_map_filter _ _ I.pendingNodup, by simp only [complete]; have := I.pendingLe; omega,
    fun p hp => I.pendingClock p (List.mem_filter.mp hp).1, I.fillsLe,
    bytes_after_complete I hk n hn, bytesLe_after_complete I hk n hn, bytesFuture_after_complete I n,
    I.ledgerNodup, I.ledgerCap, I.r2Nodup, I.r2Le, I.r2Clock, I.r2Fresh, I.tracked, I.writingNodup, ?_, ?_⟩
  · intro w hww
    obtain ⟨p, hp, hpw⟩ := List.mem_map.mp (I.writingPending w hww)
    refine List.mem_map.mpr ⟨p, List.mem_filter.mpr ⟨hp, ?_⟩, hpw⟩
    simp only [ne_eq, decide_eq_true_eq, hpw]
    intro h; exact hw (h ▸ hww)
  · intro w hww p hp hpw
    exact I.writingLive w hww p (List.mem_filter.mp hp).1 hpw

theorem upsertRow_nodup {rows : List Row} (h : (rows.map Row.key).Nodup) (row : Row) :
    ((upsertRow rows row).map Row.key).Nodup := by
  simp only [upsertRow, List.map_append, List.map_cons, List.map_nil]
  refine List.nodup_append.mpr ⟨nodup_map_filter _ _ h, by simp, ?_⟩
  intro a ha c hc
  simp at hc; subst hc
  obtain ⟨r, hr, rfl⟩ := List.mem_map.mp ha
  have := (List.mem_filter.mp hr).2
  simpa using this

theorem upsertRow_keep {rows : List Row} (row : Row) :
    ∀ r ∈ upsertRow rows row, r.key = row.key → r = row := by
  intro r hr hk
  rcases List.mem_append.mp hr with hr | hr
  · have := (List.mem_filter.mp hr).2; simp [hk] at this
  · simpa using hr

theorem inv_completeOk {P : Params} (W : P.WF) {s : State} (I : Inv P s) (k : Key) (b : Nat) (reused : Bool)
    (hk : k ∈ pendingKeys s) (hb : b ≤ P.lim.maxPackBytes) (hsize : ∀ o ∈ s.r2, o.key = k → o.bytes ≤ b) :
    Inv P (complete P s k (.ok b reused)) := by
  have hlen := complete_pending I hk
  have hn : (if reused = true then 0 else b) ≤ P.maxFillBytes := by
    split
    · exact Nat.zero_le _
    · exact Nat.le_trans hb W.pack_le_fill
  -- writing after `erase`
  have hwnd : (s.writing.erase k).Nodup := I.writingNodup.erase k
  have hwmem : ∀ w, w ≠ k → w ∈ s.writing → w ∈ s.writing.erase k :=
    fun w hne hw => (List.mem_erase_of_ne hne).mpr hw
  have hwsub : ∀ w ∈ s.writing.erase k, w ∈ s.writing ∧ w ≠ k := by
    intro w hw
    exact ⟨List.mem_of_mem_erase hw, ((I.writingNodup.mem_erase_iff).mp hw).1⟩
  have hwpend : ∀ w ∈ s.writing.erase k, w ∈ (s.pending.filter (fun p => decide (p.1 ≠ k))).map Prod.fst := by
    intro w hw
    obtain ⟨hw, hne⟩ := hwsub w hw
    obtain ⟨p, hp, hpw⟩ := List.mem_map.mp (I.writingPending w hw)
    exact List.mem_map.mpr ⟨p, List.mem_filter.mpr ⟨hp, by simp [hpw, hne]⟩, hpw⟩
  have hwlive : ∀ w ∈ s.writing.erase k, ∀ p ∈ s.pending.filter (fun p => decide (p.1 ≠ k)),
      p.1 = w → s.clock < p.2 + P.pendingTtl :=
    fun w hw p hp hpw => I.writingLive w (hwsub w hw).1 p (List.mem_filter.mp hp).1 hpw
  simp only [complete]
  split
  · -- the repository is blocked: drop the pack
    have hfilt : ∀ o ∈ s.r2.filter (fun o => decide (o.key ≠ k)), o ∈ s.r2 ∧ o.key ≠ k := by
      intro o ho; have := List.mem_filter.mp ho; simpa using this
    refine ⟨nodup_map_filter _ _ I.pendingNodup, Nat.le_trans (List.length_filter_le _ _) I.pendingLe,
      fun p hp => I.pendingClock p (List.mem_filter.mp hp).1, I.fillsLe,
      bytes_after_complete I hk _ hn, bytesLe_after_complete I hk _ hn, bytesFuture_after_complete I _,
      nodup_map_filter _ _ I.ledgerNodup, Nat.le_trans (sumBy_filter_le _ _ _) I.ledgerCap,
      nodup_map_filter _ _ I.r2Nodup, fun o ho => I.r2Le o (hfilt o ho).1,
      fun o ho => I.r2Clock o (hfilt o ho).1, fun o ho => I.r2Fresh o (hfilt o ho).1, ?_, hwnd, hwpend, hwlive⟩
    intro o ho
    obtain ⟨ho, hne⟩ := hfilt o ho
    rcases I.tracked o ho with h | ⟨row, hrow, hrk, hrb, hrc⟩
    · exact Or.inl (hwmem _ hne h)
    · exact Or.inr ⟨row, List.mem_filter.mpr ⟨hrow, by simp [hrk, hne]⟩, hrk, hrb, hrc⟩
  · -- record the pack, then evict
    let rows := upsertRow s.ledger ⟨k, b, s.clock⟩
    have hrows_nd : (rows.map Row.key).Nodup := upsertRow_nodup I.ledgerNodup _
    have hrows_len : rows.length ≤ s.ledger.length + 1 := by
      simp only [rows, upsertRow, List.length_append, List.length_singleton]
      have := List.length_filter_le (fun r : Row => decide (r.key ≠ k)) s.ledger
      omega
    have hrows_keep : ∀ r ∈ rows, r.key = k → r.bytes ≤ P.lim.storageCapBytes := by
      intro r hr hrk
      have := upsertRow_keep ⟨k, b, s.clock⟩ r hr hrk
      subst this
      exact Nat.le_trans hb W.pack_le_cap
    obtain ⟨K, hK, h1, h2, h3⟩ := evict_spec P.lim.storageCapBytes k _ rows s.r2 hrows_len hrows_nd hrows_keep
    have hr2 : ∀ o ∈ (evict P.lim.storageCapBytes k (s.ledger.length + 1) rows s.r2).2, o ∈ s.r2 ∧ o.key ∉ K := by
      intro o ho; rw [h2] at ho; have := List.mem_filter.mp ho; simpa using this
    refine ⟨nodup_map_filter _ _ I.pendingNodup, Nat.le_trans (List.length_filter_le _ _) I.pendingLe,
      fun p hp => I.pendingClock p (List.mem_filter.mp hp).1, I.fillsLe,
      bytes_after_complete I hk _ hn, bytesLe_after_complete I hk _ hn, bytesFuture_after_complete I _,
      by rw [h1]; exact nodup_map_filter _ _ hrows_nd, h3,
      by rw [h2]; exact nodup_map_filter _ _ I.r2Nodup, fun o ho => I.r2Le o (hr2 o ho).1,
      fun o ho => I.r2Clock o (hr2 o ho).1, fun o ho => I.r2Fresh o (hr2 o ho).1, ?_, hwnd, hwpend, hwlive⟩
    intro o ho
    obtain ⟨ho, hoK⟩ := hr2 o ho
    have hin : ∀ row ∈ rows, row.key = o.key →
        row ∈ (evict P.lim.storageCapBytes k (s.ledger.length + 1) rows s.r2).1 := by
      intro row hrow hrk
      rw [h1]; exact List.mem_filter.mpr ⟨hrow, by simp [hrk, hoK]⟩
    by_cases hok : o.key = k
    · refine Or.inr ⟨⟨k, b, s.clock⟩, hin _ (by simp [rows, upsertRow]) hok.symm, hok.symm,
        hsize o ho hok, I.r2Clock o ho⟩
    · rcases I.tracked o ho with h | ⟨row, hrow, hrk, hrb, hrc⟩
      · exact Or.inl (hwmem _ hok h)
      · have : row ∈ rows := List.mem_append_left _ (List.mem_filter.mpr ⟨hrow, by simp [hrk, hok]⟩)
        exact Or.inr ⟨row, hin row this hrk, hrk, hrb, hrc⟩

theorem inv_takedown {P : Params} {s : State} (I : Inv P s) (r : Nat) : Inv P (takedown P s r) := by
  have hfilt : ∀ o ∈ s.r2.filter (fun o => decide (o.key.repo ≠ r)), o ∈ s.r2 ∧ o.key.repo ≠ r := by
    intro o ho; have := List.mem_filter.mp ho; simpa using this
  refine ⟨I.pendingNodup, I.pendingLe, I.pendingClock, I.fillsLe, I.bytesToday, I.bytesLe, I.bytesFuture,
    nodup_map_filter _ _ I.ledgerNodup, Nat.le_trans (sumBy_filter_le _ _ _) I.ledgerCap,
    nodup_map_filter _ _ I.r2Nodup, fun o ho => I.r2Le o (hfilt o ho).1,
    fun o ho => I.r2Clock o (hfilt o ho).1, fun o ho => I.r2Fresh o (hfilt o ho).1, ?_,
    I.writingNodup, I.writingPending, I.writingLive⟩
  intro o ho
  obtain ⟨ho, hne⟩ := hfilt o ho
  rcases I.tracked o ho with h | ⟨row, hrow, hrk, hrb, hrc⟩
  · exact Or.inl h
  · exact Or.inr ⟨row, List.mem_filter.mpr ⟨hrow, by simp [hrk, hne]⟩, hrk, hrb, hrc⟩

theorem inv_expire {P : Params} {s : State} (I : Inv P s) (k : Key) :
    Inv P { s with r2 := s.r2.filter (fun o => decide (o.key ≠ k)) } := by
  have hfilt : ∀ o ∈ s.r2.filter (fun o => decide (o.key ≠ k)), o ∈ s.r2 := fun o ho => (List.mem_filter.mp ho).1
  exact ⟨I.pendingNodup, I.pendingLe, I.pendingClock, I.fillsLe, I.bytesToday, I.bytesLe, I.bytesFuture,
    I.ledgerNodup, I.ledgerCap, nodup_map_filter _ _ I.r2Nodup, fun o ho => I.r2Le o (hfilt o ho),
    fun o ho => I.r2Clock o (hfilt o ho), fun o ho => I.r2Fresh o (hfilt o ho),
    fun o ho => I.tracked o (hfilt o ho), I.writingNodup, I.writingPending, I.writingLive⟩

theorem inv_tick {P : Params} {s : State} (I : Inv P s) (t : Nat) (ht : s.clock ≤ t)
    (hw : ∀ k ∈ s.writing, pendingLive P s t k) : Inv P (tick P s t) := by
  have hfilt : ∀ o ∈ s.r2.filter (fun o => decide (t < o.createdAt + (P.lifecycleDays + P.lifecycleDelayDays) * day)),
      o ∈ s.r2 ∧ t < o.createdAt + (P.lifecycleDays + P.lifecycleDelayDays) * day := by
    intro o ho; have := List.mem_filter.mp ho; simpa using this
  have hday : dayOf s.clock ≤ dayOf t := Nat.div_le_div_right ht
  refine ⟨I.pendingNodup, I.pendingLe, fun p hp => Nat.le_trans (I.pendingClock p hp) ht, I.fillsLe, ?_,
    I.bytesLe, ?_, I.ledgerNodup, I.ledgerCap, nodup_map_filter _ _ I.r2Nodup,
    fun o ho => I.r2Le o (hfilt o ho).1, fun o ho => Nat.le_trans (I.r2Clock o (hfilt o ho).1) ht,
    fun o ho => (hfilt o ho).2, fun o ho => I.tracked o (hfilt o ho).1, I.writingNodup, I.writingPending, hw⟩
  · simp only [tick]
    by_cases hsame : dayOf t = dayOf s.clock
    · rw [hsame]; exact I.bytesToday
    · have hz := I.bytesFuture (dayOf t) (by omega)
      have := Nat.mul_le_mul_left P.maxFillBytes I.pendingLe
      rw [hz]; omega
  · intro d hd; exact I.bytesFuture d (by simp only [tick] at hd; omega)

theorem inv_step {P : Params} (W : P.WF) {s s' : State} (I : Inv P s) (h : Step P s s') : Inv P s' := by
  cases h with
  | request k => exact inv_request W I k
  | release k hw => exact inv_release I k hw
  | store k b hk hw hb hlive => exact inv_store W I k b hk hw hb hlive
  | completeOk k b reused hk hb hsize _ => exact inv_completeOk W I k b reused hk hb hsize
  | completeFail k r retry n hk hw hn _ => exact inv_completeFail I k r retry n hk hw hn
  | prune => exact inv_prune W I
  | takedown r => exact inv_takedown I r
  | expire k => exact inv_expire I k
  | tick t ht hw => exact inv_tick I t ht hw

theorem reachable_inv {P : Params} (W : P.WF) {s : State} (h : Reachable P s) : Inv P s := by
  induction h with
  | init => exact inv_init P
  | step _ hs ih => exact inv_step W ih hs

/-! ## Headline theorems -/

section
variable {P : Params} (W : P.WF) {s : State} (R : Reachable P s)
include W R

/-- Single-flight: at most one pending fill per `repo@commit`. -/
theorem single_flight : (pendingKeys s).Nodup := (reachable_inv W R).pendingNodup

/-- In-flight fills stay within `MAX_INFLIGHT_FILLS`; fills holding unrecorded
R2 bytes are among them. -/
theorem inflight_le : s.pending.length ≤ P.lim.maxInflightFills ∧ s.writing.length ≤ P.lim.maxInflightFills := by
  have I := reachable_inv W R
  refine ⟨I.pendingLe, Nat.le_trans ?_ I.pendingLe⟩
  have := List.Nodup.length_le_of_subset I.writingNodup (fun k hk => I.writingPending k hk)
  simpa [pendingKeys] using this

theorem daily_fills_le (d : Nat) : s.fills d ≤ P.lim.dailyFillLimit := (reachable_inv W R).fillsLe d

theorem daily_bytes_le (d : Nat) :
    s.bytes d ≤ P.lim.dailyFillBytes + P.maxFillBytes * P.lim.maxInflightFills := (reachable_inv W R).bytesLe d

theorem ledger_le_cap : sumBy Row.bytes s.ledger ≤ P.lim.storageCapBytes := (reachable_inv W R).ledgerCap

/-- R2 holds at most the cap plus one pack per fill between its write and its `complete`. -/
theorem r2_le_ledger_plus_writing :
    sumBy Obj.bytes s.r2 ≤ sumBy Row.bytes s.ledger + s.writing.length * P.lim.maxPackBytes := by
  have I := reachable_inv W R
  let inW := fun o : Obj => decide (o.key ∈ s.writing)
  have hsplit := sumBy_split Obj.bytes inW s.r2
  have hA : sumBy Obj.bytes (s.r2.filter inW) ≤ s.writing.length * P.lim.maxPackBytes := by
    have hnd : ((s.r2.filter inW).map Obj.key).Nodup := nodup_map_filter _ _ I.r2Nodup
    have hsub : (s.r2.filter inW).map Obj.key ⊆ s.writing := by
      intro k hk
      obtain ⟨o, ho, rfl⟩ := List.mem_map.mp hk
      simpa [inW] using (List.mem_filter.mp ho).2
    have hlen := List.Nodup.length_le_of_subset hnd hsub
    rw [List.length_map] at hlen
    have := sumBy_le_mul Obj.bytes P.lim.maxPackBytes (s.r2.filter inW)
      (fun o ho => I.r2Le o (List.mem_filter.mp ho).1)
    exact Nat.le_trans this (Nat.mul_le_mul_right _ hlen)
  have hB : sumBy Obj.bytes (s.r2.filter (fun o => !inW o)) ≤ sumBy Row.bytes s.ledger := by
    apply dominated_sum (nodup_map_filter _ _ I.r2Nodup)
    intro o ho
    obtain ⟨ho, hnw⟩ := List.mem_filter.mp ho
    rcases I.tracked o ho with h | ⟨row, hrow, hrk, hrb, _⟩
    · simp [inW, h] at hnw
    · exact ⟨row, hrow, hrk, hrb⟩
  omega

theorem r2_le_cap_plus_inflight :
    sumBy Obj.bytes s.r2 ≤ P.lim.storageCapBytes + P.lim.maxInflightFills * P.lim.maxPackBytes := by
  have := r2_le_ledger_plus_writing W R
  have := ledger_le_cap W R
  have := Nat.mul_le_mul_right P.lim.maxPackBytes (inflight_le W R).2
  omega

theorem r2_le_cap_when_quiescent (h : s.writing = []) : sumBy Obj.bytes s.r2 ≤ P.lim.storageCapBytes := by
  have := r2_le_ledger_plus_writing W R
  have := ledger_le_cap W R
  simp [h] at *; omega

end

/-- A fill starts only for a key that is not already pending, below the
in-flight cap, and below both daily budgets. -/
theorem queued_only_within_limits {P : Params} {s : State} (k : Key) (h : (requestFill P s k).1 = .queued) :
    k ∉ pendingKeys (prune P s) ∧ (prune P s).pending.length < P.lim.maxInflightFills ∧
      (prune P s).fills (dayOf s.clock) < P.lim.dailyFillLimit ∧
      (prune P s).bytes (dayOf s.clock) < P.lim.dailyFillBytes := by
  rcases requestFill_cases P s k with ⟨hq, _⟩ | ⟨_, hk, hlen, hf, hb, _, _⟩
  · exact absurd h hq
  · exact ⟨hk, hlen, hf, hb⟩

/-! ## Takedown -/

/-- The takedown invariant for repository `r` blocked at time `t0`. -/
structure Blocks (P : Params) (r t0 : Nat) (s : State) : Prop where
  started : t0 ≤ s.clock
  negative : s.clock < t0 + P.ttl .blocked →
    ∃ u, t0 + P.ttl .blocked ≤ u ∧ s.negatives (.repo r) = some (.blocked, u)
  noRows : s.clock < t0 + P.ttl .blocked → ∀ row ∈ s.ledger, row.key.repo ≠ r
  onlyWrites : s.clock < t0 + P.ttl .blocked → ∀ o ∈ s.r2, o.key.repo = r → o.key ∈ s.writing
  oldPending : s.clock < t0 + P.ttl .blocked → ∀ p ∈ s.pending, p.1.repo = r → p.2 ≤ t0

theorem blocks_after_takedown {P : Params} {s : State} (I : Inv P s) (r : Nat) :
    Blocks P r s.clock (takedown P s r) := by
  refine ⟨Nat.le_refl _, fun _ => ⟨_, Nat.le_refl _, by simp [takedown]⟩, ?_, ?_, ?_⟩
  · intro _ row hrow
    have := (List.mem_filter.mp hrow).2; simpa using this
  · intro _ o ho hr
    have := (List.mem_filter.mp ho).2; simp at this; exact absurd hr this
  · intro _ p hp _; exact I.pendingClock p hp

theorem blocked_of_negative {s : State} {r u : Nat} (hneg : s.negatives (.repo r) = some (.blocked, u))
    (hlt : s.clock < u) : blocked s r = true := by
  simp [blocked, hneg, hlt]

theorem live_of_negative {s : State} {r u : Nat} {why : Reason} (hneg : s.negatives (.repo r) = some (why, u))
    (hlt : s.clock < u) : live s (.repo r) = true := by
  simp [live, hneg, hlt]

theorem blocks_step {P : Params} (W : P.WF) {r t0 : Nat} {s s' : State}
    (B : Blocks P r t0 s) (h : Step P s s') : Blocks P r t0 s' := by
  cases h with
  | request k =>
    rcases requestFill_cases P s k with ⟨_, h'⟩ | ⟨_, _, _, _, _, hlive, h'⟩
    · rw [h']
      refine ⟨B.started, fun hc => ?_, fun hc row hrow => B.noRows hc row (List.mem_filter.mp hrow).1,
        B.onlyWrites, fun hc p hp => B.oldPending hc p (List.mem_filter.mp hp).1⟩
      obtain ⟨u, hu, hneg⟩ := B.negative hc
      exact ⟨u, hu, by simp [prune, hneg, show ¬ u ≤ s.clock by simp only [prune] at hc; omega]⟩
    · rw [h']
      refine ⟨B.started, fun hc => ?_, fun hc row hrow => B.noRows hc row (List.mem_filter.mp hrow).1,
        B.onlyWrites, fun hc p hp hpr => ?_⟩
      · obtain ⟨u, hu, hneg⟩ := B.negative hc
        exact ⟨u, hu, by simp [prune, hneg, show ¬ u ≤ s.clock by simp only [prune] at hc; omega]⟩
      · rcases List.mem_append.mp hp with hp | hp
        · exact B.oldPending hc p (List.mem_filter.mp hp).1 hpr
        · simp at hp; subst hp
          exfalso
          obtain ⟨u, hu, hneg⟩ := B.negative hc
          have : live (prune P s) (.repo k.repo) = true := by
            simp only at hpr
            rw [hpr]
            apply live_of_negative (u := u) (why := .blocked)
            · simp [prune, hneg, show ¬ u ≤ s.clock by simp only [prune] at hc; omega]
            · simp only [prune] at hc ⊢; omega
          simp [this] at hlive
  | release k _ =>
    exact ⟨B.started, B.negative, B.noRows, B.onlyWrites,
      fun hc p hp => B.oldPending hc p (List.mem_filter.mp hp).1⟩
  | store k b hk _ _ _ =>
    refine ⟨B.started, B.negative, B.noRows, fun hc o ho hr => ?_, B.oldPending⟩
    rcases List.mem_append.mp ho with ho | ho
    · exact List.mem_cons_of_mem _ (B.onlyWrites hc o (List.mem_filter.mp ho).1 hr)
    · simp at ho; subst ho; exact List.mem_cons_self
  | completeOk k b reused hk _ _ hlive =>
    have hsub : ∀ w, w ≠ k → w ∈ s.writing → w ∈ s.writing.erase k :=
      fun w hne hw => (List.mem_erase_of_ne hne).mpr hw
    simp only [complete]
    split
    · refine ⟨B.started, B.negative, fun hc row hrow => B.noRows hc row (List.mem_filter.mp hrow).1,
        fun hc o ho hr => ?_, fun hc p hp => B.oldPending hc p (List.mem_filter.mp hp).1⟩
      have := List.mem_filter.mp ho
      simp only [ne_eq, decide_eq_true_eq] at this
      exact hsub _ this.2 (B.onlyWrites hc o this.1 hr)
    · rename_i hnb
      have hkr : s.clock < t0 + P.ttl .blocked → k.repo ≠ r := by
        intro hc hkr
        obtain ⟨u, hu, hneg⟩ := B.negative hc
        exact hnb (by rw [hkr]; exact blocked_of_negative hneg (by omega))
      have hev := evict_sub P.lim.storageCapBytes k (s.ledger.length + 1)
        (upsertRow s.ledger ⟨k, b, s.clock⟩) s.r2
      refine ⟨B.started, B.negative, fun hc row hrow => ?_, fun hc o ho hr => ?_,
        fun hc p hp => B.oldPending hc p (List.mem_filter.mp hp).1⟩
      · rcases List.mem_append.mp (hev.1 row hrow) with h | h
        · exact B.noRows hc row (List.mem_filter.mp h).1
        · simp at h; subst h; exact hkr hc
      · exact hsub _ (fun h => hkr hc (h ▸ hr)) (B.onlyWrites hc o (hev.2 o ho) hr)
  | completeFail k why retry n hk _ _ hlive =>
    refine ⟨B.started, fun hc => ?_, B.noRows, B.onlyWrites,
      fun hc p hp => B.oldPending hc p (List.mem_filter.mp hp).1⟩
    change s.clock < _ at hc
    obtain ⟨u0, hu0, hneg⟩ := B.negative hc
    show ∃ u, _ ∧ upsertNeg s.negatives (scopeFor P k why) why (s.clock + ttlFor P why retry) (.repo r) =
      some (.blocked, u)
    by_cases hsc : Scope.repo r = scopeFor P k why
    · have hwhy : why ≠ .rateLimited ∧ P.repoScoped why = true ∧ k.repo = r := by
        unfold scopeFor at hsc
        by_cases h1 : why = .rateLimited
        · simp [h1] at hsc
        · by_cases h2 : P.repoScoped why = true
          · simp [h1, h2] at hsc; exact ⟨h1, h2, hsc.symm⟩
          · simp [h1, h2] at hsc
      obtain ⟨h1, h2, h3⟩ := hwhy
      have httl : ttlFor P why retry = P.ttl why := by simp [ttlFor, h1]
      rw [← hsc]
      simp only [upsertNeg, ite_true, hneg, httl]
      by_cases hb : why = .blocked
      · subst hb
        split
        · exact ⟨_, by omega, rfl⟩
        · exact ⟨u0, hu0, rfl⟩
      · obtain ⟨p, hp, hpk⟩ := List.mem_map.mp hk
        have hpold := B.oldPending hc p hp (by rw [hpk]; exact h3)
        have hpl := hlive p hp hpk
        have hdom := W.block_dominates why h2 hb
        rw [ite_eq_right (by omega)]
        exact ⟨u0, hu0, rfl⟩
    · exact ⟨u0, hu0, by simp only [upsertNeg, ite_eq_right hsc]; exact hneg⟩
  | prune =>
    refine ⟨B.started, fun hc => ?_, fun hc row hrow => B.noRows hc row (List.mem_filter.mp hrow).1,
      B.onlyWrites, fun hc p hp => B.oldPending hc p (List.mem_filter.mp hp).1⟩
    obtain ⟨u, hu, hneg⟩ := B.negative hc
    exact ⟨u, hu, by simp [prune, hneg, show ¬ u ≤ s.clock by simp only [prune] at hc; omega]⟩
  | takedown r' =>
    refine ⟨B.started, fun hc => ?_, fun hc row hrow => B.noRows hc row (List.mem_filter.mp hrow).1,
      fun hc o ho hr => B.onlyWrites hc o (List.mem_filter.mp ho).1 hr, B.oldPending⟩
    by_cases hrr : r = r'
    · subst hrr
      exact ⟨s.clock + P.ttl .blocked, by have := B.started; omega, by simp [takedown]⟩
    · obtain ⟨u, hu, hneg⟩ := B.negative hc
      exact ⟨u, hu, by simp [takedown, hrr, hneg]⟩
  | expire k =>
    exact ⟨B.started, B.negative, B.noRows,
      fun hc o ho hr => B.onlyWrites hc o (List.mem_filter.mp ho).1 hr, B.oldPending⟩
  | tick t ht _ =>
    refine ⟨Nat.le_trans B.started ht, fun hc => ?_, fun hc => B.noRows (by simp only [tick] at hc; omega),
      fun hc o ho hr => B.onlyWrites (by simp only [tick] at hc; omega) o (List.mem_filter.mp ho).1 hr,
      fun hc => B.oldPending (by simp only [tick] at hc; omega)⟩
    obtain ⟨u, hu, hneg⟩ := B.negative (by simp only [tick] at hc; omega)
    exact ⟨u, hu, hneg⟩

/-- Any finite sequence of events. -/
inductive Steps (P : Params) : State → State → Prop
  | refl (s : State) : Steps P s s
  | step {s s' s'' : State} : Steps P s s' → Step P s' s'' → Steps P s s''

theorem Reachable.steps {P : Params} {s s' : State} (R : Reachable P s) (h : Steps P s s') :
    Reachable P s' := by
  induction h with
  | refl => exact R
  | step _ hs ih => exact Reachable.step ih hs

/-- A takedown of `r` at time `t0` holds through every later event: until
`t0 + ttl blocked` the negative stays, no ledger row of `r` exists, every R2
object of `r` is an in-flight write, and no pending fill of `r` started after
`t0`. -/
theorem takedown_holds {P : Params} (W : P.WF) {s s' : State} (R : Reachable P s) (r : Nat)
    (h : Steps P (takedown P s r) s') : Blocks P r s.clock s' := by
  induction h with
  | refl => exact blocks_after_takedown (reachable_inv W R) r
  | step hsteps hs ih => exact blocks_step W ih hs

/-- While the block lasts, no fill of `r` is queued. -/
theorem takedown_no_fill {P : Params} {r t0 : Nat} {s : State} (B : Blocks P r t0 s)
    (hc : s.clock < t0 + P.ttl .blocked) (k : Key) (hq : (requestFill P s k).1 = .queued) : k.repo ≠ r := by
  intro hkr
  rcases requestFill_cases P s k with ⟨hn, _⟩ | ⟨_, _, _, _, _, hlive, _⟩
  · exact hn hq
  · obtain ⟨u, hu, hneg⟩ := B.negative hc
    have : live (prune P s) (.repo k.repo) = true := by
      rw [hkr]
      apply live_of_negative (u := u) (why := .blocked)
      · simp [prune, hneg, show ¬ u ≤ s.clock by omega]
      · simp only [prune]; omega
    simp [this] at hlive

end Wit.Coordinator
