import Wit.Generated.Constants
import Wit.Assumptions
import Wit.Coordinator

/-!
# Limits and cost (ADR 0009, "Limits and cost reasoning")

`Fits l` collects every arithmetic condition the cost argument of ADR 0009
needs from a set of Worker limits `l`. `deployed_fits` checks it for the limits
the Worker runs with (`DEFAULTS` overridden by wrangler `[vars]`, as
`limitsFromEnv` does) and `defaults_fits` for `DEFAULTS` alone, so raising a
cap in either place past what the free tiers allow breaks the build.

The per-day quantities come from `Wit.Coordinator` (`daily_fills_le`,
`daily_bytes_le`, `r2_le_ledger_plus_writing`); the per-operation counts come
from `services/wit-cache/src` as described on each field. A month has at most
`31 × dailyFillLimit` fills started in it plus `maxInflightFills` carried over
from the previous month.
-/

namespace Wit.Budget

open Wit

/-- `limitsFromEnv(env)` for the deployed Worker. -/
def deployed : Limits := Src.workerDefaults.override Src.wranglerVars

/-- Most bytes one fill reports: the pack cap plus one side-band chunk
(`MAX_PKT_LEN` minus the 4-byte length and the band byte), because
`PackVerifier.update` counts a chunk before rejecting it. -/
def maxFillBytes (l : Limits) : Nat := l.maxPackBytes + (Src.maxPktLen - 5)

/-- `Coordinator.daily_bytes_le`: bytes counted in one UTC day. -/
def dailyBytesBound (l : Limits) : Nat := l.dailyFillBytes + maxFillBytes l * l.maxInflightFills

/-- Fill messages in one month: the coordinator enqueues one per counted fill. -/
def monthlyFills (l : Limits) : Nat := Assume.monthDays * l.dailyFillLimit + l.maxInflightFills

/-- Runs of one queued message: the first delivery plus `max_retries`. -/
def runsPerMessage : Nat := 1 + Src.queueMaxRetries

/-- R2 Class A operations of one `PackUpload` of `n` bytes with parts of `p`
bytes: one PUT below `p`, otherwise create + one per part + complete. -/
def uploadClassA (p n : Nat) : Nat :=
  if n < p then 1 else n / p + (if n % p = 0 then 0 else 1) + 2

theorem uploadClassA_le (p n : Nat) : uploadClassA p n ≤ n / p + 3 := by
  unfold uploadClassA
  have := Nat.zero_le (n / p)
  split
  · omega
  · split <;> omega

/-- A failed upload has created the multipart upload and sent at most
`n / p` parts before `abort` (which is free). -/
def failedUploadClassA (p n : Nat) : Nat := n / p + 1

/-- What one run of a fill job writes to R2: `fillPack`'s HEAD finds the pack
already stored (free of Class A), or an upload of `n` verified bytes completes
or aborts. -/
inductive Run where
  | reused
  | stored (n : Nat)
  | failed (n : Nat)

def Run.bytes : Run → Nat
  | .reused => 0
  | .stored n => n
  | .failed n => n

def Run.classA (p : Nat) : Run → Nat
  | .reused => 0
  | .stored n => uploadClassA p n
  | .failed n => failedUploadClassA p n

/-- Class A operations of the largest run: an upload of `maxFillBytes`. -/
def maxRunClassA (l : Limits) : Nat := maxFillBytes l / l.partBytes + 3

theorem run_classA_le (l : Limits) (r : Run) (h : r.bytes ≤ maxFillBytes l) :
    r.classA l.partBytes ≤ maxRunClassA l := by
  have hd : r.bytes / l.partBytes ≤ maxFillBytes l / l.partBytes := Nat.div_le_div_right h
  unfold maxRunClassA
  cases r with
  | reused => simp [Run.classA]
  | stored n =>
    have := uploadClassA_le l.partBytes n
    simp only [Run.classA]
    simp only [Run.bytes] at hd
    omega
  | failed n =>
    simp only [Run.classA, failedUploadClassA]
    simp only [Run.bytes] at hd
    omega

/-- Worst-case R2 Class A operations in a month: the baseline plus every run of
every message at the largest pack. Retried runs are included and the byte
budget is not used, because a run whose `complete` failed never counted. -/
def classAWorstCase (l : Limits) : Nat :=
  Assume.baselineClassA + runsPerMessage * monthlyFills l * maxRunClassA l

theorem sum_le_length_mul (c : Nat) : ∀ xs : List Nat, (∀ x ∈ xs, x ≤ c) → xs.sum ≤ xs.length * c
  | [], _ => by simp
  | x :: xs, h => by
    have ih := sum_le_length_mul c xs (fun y hy => h y (by simp [hy]))
    have hx := h x (by simp)
    simp only [List.sum_cons, List.length_cons, Nat.succ_mul]
    omega

/-- A month has at most `monthlyFills` messages (`daily_fills_le`), the queue
runs each at most `runsPerMessage` times, and no run uploads more than
`maxFillBytes`; so its Class A operations are at most `classAWorstCase`. -/
theorem month_classA_le (l : Limits) (runs : List Run)
    (hlen : runs.length ≤ runsPerMessage * monthlyFills l)
    (hbytes : ∀ r ∈ runs, r.bytes ≤ maxFillBytes l) :
    Assume.baselineClassA + (runs.map (Run.classA l.partBytes)).sum ≤ classAWorstCase l := by
  have h := sum_le_length_mul (maxRunClassA l) (runs.map (Run.classA l.partBytes)) (by
    intro x hx
    obtain ⟨r, hr, rfl⟩ := List.mem_map.mp hx
    exact run_classA_le l r (hbytes r hr))
  rw [List.length_map] at h
  have := Nat.mul_le_mul_right (maxRunClassA l) hlen
  exact Nat.add_le_add_left (Nat.le_trans h this) _

/-- The coordinator parameters the deployed Worker runs with. -/
def params (l : Limits) : Coordinator.Params where
  lim := l
  pendingTtl := Src.pendingTtlSeconds
  graceDays := Src.ledgerGraceDays
  lifecycleDays := Src.lifecycleExpireDays
  lifecycleDelayDays := Assume.lifecycleDelayDays
  maxFillBytes := maxFillBytes l
  ttl := Src.negativeTtl
  repoScoped := Src.repoScoped
  rateCap := Src.rateLimitedTtlCap

/-- Client requests a month for which requests and Class B stay in the allowance. -/
def clientRequestHeadroom : Nat := 9500000

/-- Worker CPU left for serving requests after fills. -/
def requestCpuHeadroomMs : Nat := 20000000

structure Fits (l : Limits) : Prop where
  /-- Eviction can always make room: one pack is at most the cap. -/
  packLeCap : l.maxPackBytes ≤ l.storageCapBytes
  /-- ADR 0009: five packs fit in the store (six do not, see `six_packs_exceed_cap`). -/
  fivePacksFit : 5 * l.maxPackBytes ≤ l.storageCapBytes
  /-- The client asks for no more than the Worker stores. -/
  clientCapMatches : Src.clientDefaultMaxBytes = l.maxPackBytes
  /-- Steady state: the ledger cap plus the other buckets leaves 1 GB of the free tier. -/
  storageWithHeadroom :
    Assume.baselineStorageBytes + l.storageCapBytes + 1000000000 ≤ Assume.r2FreeStorageBytes
  /-- Monthly average storage. Bytes outside the ledger are a fill's upload between
  its first part and its `complete`, all inside one consumer invocation, so each
  counted byte spends at most `queueConsumerWallMs` outside the cap. Averaged over
  the month: `baseline + cap + dailyBytesBound × wall / day ≤ 10 GB`. -/
  storageMonthlyAverage :
    (Assume.baselineStorageBytes + l.storageCapBytes) * (86400 * 1000) +
        dailyBytesBound l * Assume.queueConsumerWallMs ≤
      Assume.r2FreeStorageBytes * (86400 * 1000)
  /-- R2 Class A, queue retries included (`month_classA_le`). -/
  classA : classAWorstCase l ≤ Assume.r2FreeClassA
  /-- R2 Class B: one GET or HEAD per client request, one HEAD per fill run. -/
  classB :
    Assume.baselineClassB + clientRequestHeadroom + runsPerMessage * monthlyFills l ≤ Assume.r2FreeClassB
  /-- Workers requests: one per client request, one queue invocation per fill run. -/
  requests :
    Assume.baselineRequests + clientRequestHeadroom + runsPerMessage * monthlyFills l ≤
      Assume.workersIncludedRequests
  /-- Workers CPU: fills at 14 ms/MB over a month of counted bytes leave 20M CPU-ms. -/
  cpu :
    Assume.baselineCpuMs * 1000000 + Assume.monthDays * dailyBytesBound l * Assume.fillCpuMsPerMB +
        requestCpuHeadroomMs * 1000000 ≤ Assume.workersIncludedCpuMs * 1000000
  /-- The largest fill fits in one invocation's `cpu_ms`. -/
  fillCpu : maxFillBytes l * Assume.fillCpuMsPerMB ≤ Src.cpuMsPerInvocation * 1000000
  /-- Queues: a write and an ack per message plus one read per run. -/
  queueOps : (Assume.queueOpsPerMessage + runsPerMessage) * monthlyFills l ≤ Assume.queuesIncludedOps
  /-- One part buffer per running consumer fits in isolate memory. -/
  partMemory : Src.queueMaxConcurrency * Src.queueMaxBatchSize * l.partBytes ≤ Assume.isolateMemoryBytes
  /-- R2 multipart: part size within limits and the largest pack within the part count. -/
  partMin : Assume.r2MinPartBytes ≤ l.partBytes
  partMax : l.partBytes ≤ Assume.r2MaxPartBytes
  partCount : l.maxPackBytes / l.partBytes + 1 ≤ Assume.r2MaxParts
  /-- A fill times out and aborts its upload inside the consumer invocation. -/
  timeoutInsideWall : l.fillTimeoutMs ≤ Assume.queueConsumerWallMs
  /-- Delivery plus the consumer run end before the pending row expires, so the
  coordinator model's timing preconditions hold. -/
  runInsidePending : Assume.queueDeliveryMaxMs + Assume.queueConsumerWallMs ≤ Src.pendingTtlSeconds * 1000
  /-- The R2 lifecycle rule expires packs at the retention the ledger uses. -/
  lifecycleMatches : Src.lifecycleExpireDays = l.retentionDays
  /-- The coordinator's in-flight cap matches the queue's concurrency. -/
  inflightMatchesQueue : Src.queueMaxConcurrency ≤ l.maxInflightFills

theorem deployed_fits : Fits deployed := by
  constructor <;> decide

theorem defaults_fits : Fits Src.workerDefaults := by
  constructor <;> decide

/-- ADR 0009: a month's R2 Class A operations, every queue retry included, stay
within the 1M free tier for any limits that pass `Fits`. -/
theorem classA_worst_case_le_free_tier (l : Limits) (h : Fits l) (runs : List Run)
    (hlen : runs.length ≤ runsPerMessage * monthlyFills l)
    (hbytes : ∀ r ∈ runs, r.bytes ≤ maxFillBytes l) :
    Assume.baselineClassA + (runs.map (Run.classA l.partBytes)).sum ≤ Assume.r2FreeClassA :=
  Nat.le_trans (month_classA_le l runs hlen hbytes) h.classA

/-- 9,304 messages × 2 runs × 35 operations + the 60k baseline. -/
theorem deployed_classA_worst_case : classAWorstCase deployed = 711280 := by decide

theorem defaults_classA_worst_case : classAWorstCase Src.workerDefaults = 711280 := by decide

/-- ADR 0009 once said one pack is at most 1/6 of the store; it is not. -/
theorem six_packs_exceed_cap : Src.workerDefaults.storageCapBytes < 6 * Src.workerDefaults.maxPackBytes := by
  decide

/-- The side conditions of the coordinator model hold for the real parameters. -/
theorem params_wf (l : Limits) (h : Fits l) : (params l).WF where
  pack_le_cap := h.packLeCap
  pack_le_fill := by simp [params, maxFillBytes]
  ledger_outlives := by
    simp only [params]
    rw [h.lifecycleMatches]
    have : Assume.lifecycleDelayDays ≤ Src.ledgerGraceDays := by decide
    omega
  lifecycle_pos := by simp only [params]; decide
  block_dominates := by
    intro r hr hb
    cases r <;> simp_all [params] <;> decide

theorem deployed_wf : (params deployed).WF := params_wf deployed deployed_fits

section
open Coordinator
variable {s : State} (R : Reachable (params deployed) s)
include R

/-- The ledger never exceeds the 3 GB cap. -/
theorem deployed_ledger_le : sumBy Row.bytes s.ledger ≤ 3000000000 :=
  ledger_le_cap deployed_wf R

/-- R2 never holds more than the cap plus four packs mid-write (5.15 GB). -/
theorem deployed_r2_peak : sumBy Obj.bytes s.r2 ≤ 3000000000 + 4 * 536870912 :=
  r2_le_cap_plus_inflight deployed_wf R

/-- Bytes counted per UTC day stay under 8 GB plus four fills' overshoot. -/
theorem deployed_daily_bytes (d : Nat) : s.bytes d ≤ dailyBytesBound deployed :=
  daily_bytes_le deployed_wf R d

theorem deployed_daily_fills (d : Nat) : s.fills d ≤ 300 :=
  daily_fills_le deployed_wf R d

end

end Wit.Budget
