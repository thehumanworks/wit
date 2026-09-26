/-!
# Streaming pack verification and R2 upload (ADR 0009)

Models `PackVerifier` and `PackUpload` in `services/wit-cache/src/store.js`.
Both see the pack as an arbitrary sequence of chunks (side-band payloads), so
the theorems quantify over every chunking of the stream.

* `verifier_state`: after any chunking, the bytes fed to SHA-1 are exactly the
  stream minus its last 20 bytes, the held-back tail is exactly those 20 bytes,
  and the header is the first 12 bytes. So the trailer check compares the right
  digest with the right bytes however GitHub splits the stream.
* `verifier_accepts_le_cap`: a stream the verifier lets through is at most
  `MAX_PACK_BYTES` long.
* `upload_object_eq_stream`: the R2 object is exactly the stream, every
  multipart part except the last has exactly `PART_BYTES` bytes (an R2
  requirement), and a single PUT is used exactly when the stream is shorter
  than one part.
-/

namespace Wit.Streaming

abbrev Bytes := List Nat

/-- Length of the SHA-1 trailer held back from the digest. -/
def trailerLen : Nat := 20
/-- `PACK`, version, object count. -/
def headerLen : Nat := 12

structure Verifier where
  /-- Bytes passed to `hash.update`, in order. -/
  hashed : Bytes
  tail : Bytes
  head : Bytes
  bytes : Nat

def Verifier.init : Verifier := ⟨[], [], [], 0⟩

/-- `PackVerifier.update` without the size check (see `feed`). -/
def Verifier.update (v : Verifier) (chunk : Bytes) : Verifier :=
  let head := if v.head.length < headerLen then v.head ++ chunk.take (headerLen - v.head.length) else v.head
  let bytes := v.bytes + chunk.length
  if trailerLen ≤ chunk.length then
    { hashed := v.hashed ++ v.tail ++ chunk.take (chunk.length - trailerLen)
      tail := chunk.drop (chunk.length - trailerLen), head, bytes }
  else
    let joined := v.tail ++ chunk
    let cut := joined.length - trailerLen
    { hashed := v.hashed ++ joined.take cut, tail := joined.drop cut, head, bytes }

/-- `PackVerifier.update` with the size check: `none` is the `too_large` throw. -/
def Verifier.feed (maxBytes : Nat) : Option Verifier → Bytes → Option Verifier
  | none, _ => none
  | some v, chunk =>
    let v' := v.update chunk
    if v'.bytes > maxBytes then none else some v'

/-- The invariant `update` maintains, stated against the whole stream seen so far. -/
def Verifier.Sound (v : Verifier) (all : Bytes) : Prop :=
  v.hashed ++ v.tail = all ∧ v.tail.length = min trailerLen all.length ∧
    v.head = all.take headerLen ∧ v.bytes = all.length

theorem Verifier.init_sound : Verifier.init.Sound [] := by
  simp [Verifier.init, Verifier.Sound]

theorem Verifier.update_sound {v : Verifier} {all : Bytes} (h : v.Sound all) (chunk : Bytes) :
    (v.update chunk).Sound (all ++ chunk) := by
  obtain ⟨hcat, htail, hhead, hbytes⟩ := h
  have hhead' : (if v.head.length < headerLen then v.head ++ chunk.take (headerLen - v.head.length)
      else v.head) = (all ++ chunk).take headerLen := by
    rw [List.take_append, hhead]
    by_cases hlt : all.length < headerLen
    · have : (all.take headerLen).length = all.length := by simp; omega
      simp only [this, hlt, ite_true]
    · have hl : (all.take headerLen).length = headerLen := by simp; omega
      have : headerLen - all.length = 0 := by omega
      simp [hl, this]
  unfold Verifier.update
  by_cases hc : trailerLen ≤ chunk.length
  · simp only [hc, ite_true]
    refine ⟨?_, ?_, hhead', ?_⟩
    · simp only [List.append_assoc, List.take_append_drop]
      rw [← hcat, List.append_assoc]
    · simp only [List.length_drop, List.length_append, trailerLen] at hc ⊢; omega
    · simp [hbytes]
  · simp only [hc, ite_false]
    refine ⟨?_, ?_, hhead', ?_⟩
    · simp only [List.append_assoc, List.take_append_drop]
      rw [← hcat, List.append_assoc]
    · simp only [List.length_drop, List.length_append, htail, trailerLen] at hc ⊢; omega
    · simp [hbytes]

/-- Run the verifier over a chunked stream (no size check). -/
def Verifier.run (chunks : List Bytes) : Verifier := chunks.foldl Verifier.update Verifier.init

theorem Verifier.foldl_sound (chunks : List Bytes) :
    ∀ (v : Verifier) (all : Bytes), v.Sound all →
      (chunks.foldl Verifier.update v).Sound (all ++ chunks.flatten) := by
  induction chunks with
  | nil => intro v all h; simpa using h
  | cons c cs ih =>
    intro v all h
    have := ih (v.update c) (all ++ c) (Verifier.update_sound h c)
    simpa [List.flatten_cons, List.append_assoc] using this

/-- For every chunking: the digest input is the stream minus the trailer, the
checked trailer is the last 20 bytes, and the header is the first 12 bytes. -/
theorem verifier_state (chunks : List Bytes) :
    (Verifier.run chunks).hashed = chunks.flatten.take (chunks.flatten.length - trailerLen) ∧
      (Verifier.run chunks).tail = chunks.flatten.drop (chunks.flatten.length - trailerLen) ∧
      (Verifier.run chunks).head = chunks.flatten.take headerLen ∧
      (Verifier.run chunks).bytes = chunks.flatten.length := by
  have h := Verifier.foldl_sound chunks Verifier.init [] Verifier.init_sound
  simp only [List.nil_append] at h
  unfold Verifier.run
  generalize chunks.foldl Verifier.update Verifier.init = v at *
  generalize chunks.flatten = all at *
  obtain ⟨hcat, htail, hhead, hbytes⟩ := h
  subst hcat
  have hl : (v.hashed ++ v.tail).length - trailerLen = v.hashed.length := by
    simp only [List.length_append, trailerLen] at htail ⊢; omega
  refine ⟨?_, ?_, hhead, hbytes⟩
  · rw [hl, List.take_left]
  · rw [hl, List.drop_left]

theorem Verifier.feed_none (maxBytes : Nat) (chunks : List Bytes) :
    chunks.foldl (Verifier.feed maxBytes) none = none := by
  induction chunks with
  | nil => rfl
  | cons c cs ih => simpa [List.foldl, Verifier.feed] using ih

/-- With the size check, an accepted stream is exactly `run` and fits the cap. -/
theorem verifier_accepts_le_cap (maxBytes : Nat) (chunks : List Bytes) :
    ∀ (v : Verifier), v.bytes ≤ maxBytes →
      ∀ w, chunks.foldl (Verifier.feed maxBytes) (some v) = some w →
        w = chunks.foldl Verifier.update v ∧ w.bytes ≤ maxBytes := by
  induction chunks with
  | nil => intro v hv w h; simp at h; subst h; exact ⟨rfl, hv⟩
  | cons c cs ih =>
    intro v hv w h
    simp only [List.foldl, Verifier.feed] at h
    by_cases hover : (v.update c).bytes > maxBytes
    · simp only [hover, ite_true, Verifier.feed_none] at h; cases h
    · simp only [hover, ite_false] at h
      exact ih (v.update c) (by omega) w h

/-! ## `PackUpload` -/

structure Upload where
  /-- The part buffer (`this.part.subarray(0, this.fill)`). -/
  buf : Bytes
  /-- Parts already sent with `uploadPart`, in order. -/
  parts : List Bytes

/-- `write` copies bytes into the part buffer and flushes it when it is full;
byte-at-a-time is the same function as the `subarray` loop. -/
def Upload.writeByte (partBytes : Nat) (u : Upload) (b : Nat) : Upload :=
  if (u.buf ++ [b]).length = partBytes then ⟨[], u.parts ++ [u.buf ++ [b]]⟩ else ⟨u.buf ++ [b], u.parts⟩

def Upload.write (partBytes : Nat) (u : Upload) (chunk : Bytes) : Upload :=
  chunk.foldl (Upload.writeByte partBytes) u

/-- `finish`: a single PUT when no part was flushed, else the parts plus the
non-empty remainder. -/
def Upload.finish (u : Upload) : Bool × List Bytes :=
  if u.parts = [] then (true, [u.buf]) else (false, u.parts ++ (if u.buf = [] then [] else [u.buf]))

def Upload.Sound (partBytes : Nat) (u : Upload) (all : Bytes) : Prop :=
  u.parts.flatten ++ u.buf = all ∧ (∀ p ∈ u.parts, p.length = partBytes) ∧ u.buf.length < partBytes

theorem Upload.writeByte_sound {p : Nat} {u : Upload} {all : Bytes} (h : u.Sound p all) (b : Nat) :
    (u.writeByte p b).Sound p (all ++ [b]) := by
  obtain ⟨hcat, hparts, hbuf⟩ := h
  unfold Upload.writeByte
  by_cases hfull : (u.buf ++ [b]).length = p
  · simp only [hfull, ite_true]
    refine ⟨?_, ?_, ?_⟩
    · simp [← hcat, List.flatten_append, List.append_assoc]
    · intro q hq
      simp only [List.mem_append, List.mem_singleton] at hq
      rcases hq with hq | hq
      · exact hparts q hq
      · subst hq; exact hfull
    · simp at hfull ⊢; omega
  · simp only [hfull, ite_false]
    refine ⟨?_, hparts, ?_⟩
    · simp [← hcat, List.append_assoc]
    · simp at hfull ⊢; omega

theorem Upload.write_sound {p : Nat} (chunk : Bytes) :
    ∀ (u : Upload) (all : Bytes), u.Sound p all → (u.write p chunk).Sound p (all ++ chunk) := by
  induction chunk with
  | nil => intro u all h; simpa [Upload.write] using h
  | cons b bs ih =>
    intro u all h
    have := ih (u.writeByte p b) (all ++ [b]) (Upload.writeByte_sound h b)
    simpa [Upload.write, List.foldl, List.append_assoc] using this

theorem Upload.writeAll_sound {p : Nat} (hp : 0 < p) (chunks : List Bytes) :
    (chunks.foldl (Upload.write p) ⟨[], []⟩).Sound p chunks.flatten := by
  suffices ∀ (u : Upload) (all : Bytes), u.Sound p all →
      (chunks.foldl (Upload.write p) u).Sound p (all ++ chunks.flatten) by
    simpa using this ⟨[], []⟩ [] ⟨rfl, by simp, by simpa using hp⟩
  induction chunks with
  | nil => intro u all h; simpa using h
  | cons c cs ih =>
    intro u all h
    have := ih (u.write p c) (all ++ c) (Upload.write_sound c u all h)
    simpa [List.flatten_cons, List.append_assoc] using this

def Upload.run (partBytes : Nat) (chunks : List Bytes) : Upload :=
  chunks.foldl (Upload.write partBytes) ⟨[], []⟩

theorem Upload.flatten_length {p : Nat} (ps : List Bytes) (hps : ∀ q ∈ ps, q.length = p) :
    ps.flatten.length = ps.length * p := by
  induction ps with
  | nil => simp
  | cons q qs ih =>
    simp only [List.flatten_cons, List.length_append, List.length_cons]
    rw [hps q (by simp), ih (fun r hr => hps r (by simp [hr])), Nat.succ_mul]
    omega

/-- The stored object is the stream; parts before the last are exactly
`partBytes`; the last is at most `partBytes`; single PUT iff the stream is
shorter than a part; and the number of full parts is `length / partBytes`. -/
theorem upload_object_eq_stream (p : Nat) (hp : 0 < p) (chunks : List Bytes) :
    (Upload.run p chunks).finish.2.flatten = chunks.flatten ∧
      (∀ q ∈ (Upload.run p chunks).parts, q.length = p) ∧ (Upload.run p chunks).buf.length < p ∧
      ((Upload.run p chunks).finish.1 = true ↔ chunks.flatten.length < p) ∧
      (Upload.run p chunks).parts.length = chunks.flatten.length / p := by
  have h := Upload.writeAll_sound hp chunks
  unfold Upload.run
  generalize chunks.foldl (Upload.write p) ⟨[], []⟩ = u at *
  generalize chunks.flatten = all at *
  obtain ⟨hcat, hparts, hbuf⟩ := h
  subst hcat
  have hlen : (u.parts.flatten ++ u.buf).length = u.parts.length * p + u.buf.length := by
    rw [List.length_append, Upload.flatten_length _ hparts]
  refine ⟨?_, hparts, hbuf, ?_, ?_⟩
  · unfold Upload.finish
    by_cases h0 : u.parts = []
    · simp [h0]
    · by_cases hb : u.buf = [] <;> simp [h0, hb]
  · unfold Upload.finish
    by_cases h0 : u.parts = []
    · simp [h0, hbuf]
    · have hpos : 0 < u.parts.length := List.length_pos_iff.mpr h0
      have : p ≤ u.parts.length * p := Nat.le_mul_of_pos_left p hpos
      simp only [h0, ite_false, Bool.false_eq_true, false_iff]
      omega
  · rw [hlen, Nat.add_comm, Nat.add_mul_div_right _ _ hp, Nat.div_eq_of_lt hbuf, Nat.zero_add]

end Wit.Streaming
