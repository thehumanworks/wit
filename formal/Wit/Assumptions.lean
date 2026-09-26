/-!
# Facts about the platform and the account (not in the repository)

Every number here comes from Cloudflare's published limits or from the account
baseline recorded in ADR 0009. They are the only hand-written numbers the
budget proofs use; `scripts/check_formal.sh` cannot check them against code,
so a change in Cloudflare pricing or in the account's other usage has to be
made here by hand. Where the source gives a range, the value is the
conservative end.
-/

namespace Wit.Assume

/-! ## R2 free tier (account-wide, per month) -/

/-- 10 GB-month of storage, in bytes. -/
def r2FreeStorageBytes : Nat := 10000000000
def r2FreeClassA : Nat := 1000000
def r2FreeClassB : Nat := 10000000

/-! ## Workers Paid (included per month) -/

def workersIncludedRequests : Nat := 10000000
def workersIncludedCpuMs : Nat := 30000000
def queuesIncludedOps : Nat := 1000000
/-- Wall-clock limit of one queue consumer invocation (15 min). -/
def queueConsumerWallMs : Nat := 900000
/-- Worker isolate memory. -/
def isolateMemoryBytes : Nat := 128000000

/-! ## R2 multipart rules -/

def r2MinPartBytes : Nat := 5 * 1024 * 1024
def r2MaxPartBytes : Nat := 5 * 1024 * 1024 * 1024
def r2MaxParts : Nat := 10000

/-- R2 applies an expiry lifecycle rule within a day of the object expiring. -/
def lifecycleDelayDays : Nat := 1

/-- A queued message reaches a consumer within 5 min. With the 15 min consumer
limit this keeps every R2 write and `complete` inside the 20 min pending TTL,
which is the timing the coordinator model assumes. -/
def queueDeliveryMaxMs : Nat := 300000

/-- Queue operations per delivered message beyond its reads: one write and one ack. -/
def queueOpsPerMessage : Nat := 2

/-! ## Account baseline (ADR 0009, 2026-09) -/

/-- "5.5 GB" of other buckets, read as GiB to stay on the safe side. -/
def baselineStorageBytes : Nat := 5905580032
def baselineClassA : Nat := 60000
def baselineClassB : Nat := 90000
def baselineRequests : Nat := 400000
def baselineCpuMs : Nat := 5500000

/-! ## Measured fill cost (ADR 0009) -/

/-- Worker CPU per MB of pack filled, top of the measured 9–14 ms/MB. -/
def fillCpuMsPerMB : Nat := 14

/-- Days in the longest month. -/
def monthDays : Nat := 31

end Wit.Assume
