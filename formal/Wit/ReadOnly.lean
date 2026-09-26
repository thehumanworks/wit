import Wit.Generated.Constants

/-!
# Clients are read-only and carry no credentials (ADR 0009)

The generator lists, from the source, every caller header the Worker reads,
every method it routes, the headers it sends upstream, and the requests the
Rust client makes. These theorems pin those lists:

* `client_sends_no_credentials`: the client only issues `GET`, sends only a
  `User-Agent`, and follows no redirects (so nothing it sends can be replayed
  to another host).
* `worker_ignores_authorization`: a Worker whose response depends only on the
  method, the URL, and the caller headers it reads answers the same whatever
  `Authorization` header a caller sends; nothing a caller authenticates with
  changes what it gets.
* `worker_upstream_anonymous`: fills fetch from GitHub without an
  `Authorization` header and without following redirects.
* `only_takedown_mutates`: the only non-read method is `DELETE`, which
  `handleTakedown` gates on `X-Wit-Admin-Key`.
-/

namespace Wit.ReadOnly

theorem client_sends_no_credentials :
    Src.clientMethods = ["GET"] ∧ "authorization" ∉ Src.clientHeaders ∧
      Src.clientHeaders ⊆ ["user-agent"] ∧ Src.clientRedirect = "none" := by
  decide

/-- A caller request as the Worker sees it. Header names are lowercase. -/
structure Request where
  method : String
  url : String
  headers : String → Option String

/-- The part of a request the Worker's code reads. -/
def observed (r : Request) : String × String × List (Option String) :=
  (r.method, r.url, Src.callerHeadersRead.map r.headers)

def withHeader (r : Request) (name : String) (v : Option String) : Request :=
  { r with headers := fun n => if n = name then v else r.headers n }

theorem authorization_not_read : "authorization" ∉ Src.callerHeadersRead := by decide

/-- Any handler that reads only `observed` answers the same for every
`Authorization` header. -/
theorem worker_ignores_authorization {Resp : Type} (handle : String × String × List (Option String) → Resp)
    (r : Request) (v : Option String) :
    handle (observed (withHeader r "authorization" v)) = handle (observed r) := by
  have : Src.callerHeadersRead.map (withHeader r "authorization" v).headers =
      Src.callerHeadersRead.map r.headers := by
    apply List.map_congr_left
    intro n hn
    have : n ≠ "authorization" := fun h => authorization_not_read (h ▸ hn)
    simp [withHeader, this]
  unfold observed
  rw [this]
  rfl

theorem worker_upstream_anonymous :
    "authorization" ∉ Src.upstreamHeaders ∧ Src.upstreamRedirect = "manual" := by
  decide

theorem only_takedown_mutates :
    Src.workerMethods.filter (fun m => m ≠ "GET" ∧ m ≠ "HEAD") = ["DELETE"] ∧
      "x-wit-admin-key" ∈ Src.callerHeadersRead := by
  decide

/-- Workers observability (persisted logs) stays off. -/
theorem no_persisted_logs : Src.observabilityEnabled = false := by decide

end Wit.ReadOnly
