/-!
# Shared vocabulary for the models

Types that both the generated constants (`Wit.Generated.Constants`) and the
proofs use. Nothing here states a fact about the repository; the facts live in
the generated file and the theorems.
-/

namespace Wit

/-- Fill failure reasons, one per key of `NEGATIVE_TTL` in
`services/wit-cache/src/config.js`. The generator maps each JS key to a
constructor, so adding or removing a reason in JS breaks the build until the
model is updated. -/
inductive Reason where
  | notFoundOrPrivate
  | tooLarge
  | rateLimited
  | notTip
  | upstreamError
  | badPack
  | timeout
  | blocked
  deriving DecidableEq, Repr

/-- The Worker limits of `DEFAULTS` in `services/wit-cache/src/config.js`. -/
structure Limits where
  maxPackBytes : Nat
  storageCapBytes : Nat
  dailyFillLimit : Nat
  dailyFillBytes : Nat
  maxInflightFills : Nat
  partBytes : Nat
  fillTimeoutMs : Nat
  retentionDays : Nat
  deriving Repr

/-- Wrangler `[vars]` entries: `some n` overrides the default with the same name. -/
structure LimitVars where
  maxPackBytes : Option Nat := none
  storageCapBytes : Option Nat := none
  dailyFillLimit : Option Nat := none
  dailyFillBytes : Option Nat := none
  maxInflightFills : Option Nat := none
  partBytes : Option Nat := none
  fillTimeoutMs : Option Nat := none
  retentionDays : Option Nat := none
  deriving Repr

/-- `limitsFromEnv`: a var overrides the default only when it is a positive number. -/
def overrideWith (var : Option Nat) (default : Nat) : Nat :=
  match var with
  | some n => if 0 < n then n else default
  | none => default

def Limits.override (d : Limits) (v : LimitVars) : Limits where
  maxPackBytes := overrideWith v.maxPackBytes d.maxPackBytes
  storageCapBytes := overrideWith v.storageCapBytes d.storageCapBytes
  dailyFillLimit := overrideWith v.dailyFillLimit d.dailyFillLimit
  dailyFillBytes := overrideWith v.dailyFillBytes d.dailyFillBytes
  maxInflightFills := overrideWith v.maxInflightFills d.maxInflightFills
  partBytes := overrideWith v.partBytes d.partBytes
  fillTimeoutMs := overrideWith v.fillTimeoutMs d.fillTimeoutMs
  retentionDays := overrideWith v.retentionDays d.retentionDays

end Wit
