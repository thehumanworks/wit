import Wit.Generated.Constants
import Wit.ReadOnly

/-!
# Where a read comes from, and what `wit --help` says about it

Models how the CLI picks a snapshot backend, whether the shared cloud pack
cache (ADR 0009) is on, and which source serves a disk read, then proves the
claims of the help text against that model. Both sides are extracted by
`scripts/gen_formal_constants.mjs`:

* **code** (`Src.backend*`, `Src.*DefaultUrl`, `Src.clientDisableValues`,
  `Src.diskReadOrder`, `Src.memoryUses*`) from `CliSnapshotBackend` in
  `crates/wit/src/snapshot/mod.rs`, `built_in_default` / `from_values` /
  `is_disabled` in `crates/wit/src/gitops/cloud.rs`, and the fill order of
  `cache_github_repo_target{,_with_context}`, `recache_repo`, and
  `refresh_repo_with_context` in `crates/wit/src/gitops/ops.rs`. The generator
  also fails unless the code keeps the shape modeled here (flag before
  environment, blank environment ignored, values trimmed and ASCII-lowercased,
  a usable local entry served before any fill, a verified cloud fill returned
  before cloning);
* **help** (`Src.help*`) from `ROOT_AFTER_HELP`, `BACKEND_HELP`, and
  `BRANCHES_BACKEND_HELP` in `crates/wit/src/cli.rs`.

Changing a default or the fill order in the code, or the matching sentence of
the help, without the other makes `check_formal.sh` fail: either the constants
are stale or one of these theorems no longer holds.

* `default_backend_matches_help`: with no `--backend` and
  `WIT_SNAPSHOT_BACKEND` unset or blank, the backend is the one the help and
  every `--backend` flag doc call the default (disk).
* `flag_wins_over_env`, `documented_backends_parse`: `--backend` beats the
  variable, and `disk` / `memory` parse from either, in any case.
* `release_default_matches_help`, `debug_default_matches_help`: with nothing
  set and nothing baked at build time, release builds use the URL the help
  names and debug builds leave the cloud cache off.
* `env_url_enables_any_build`: a valid, non-disabling `WIT_CACHE_URL` turns it
  on in every build, whatever the baked default.
* `disable_values_match_help`, `help_disable_values_disable`: the help lists
  exactly the values that turn the cache off, and each (also upper-cased) does
  in every build.
* `disk_read_order_matches_help`, `warm_cache_served_locally`,
  `cloud_first_when_enabled`, `falls_back_to_github`, `cloud_only_when_enabled`,
  `debug_build_never_cloud`, `release_build_tries_cloud`: a disk read is served
  from the local cache, then the cloud pack, then a GitHub clone, exactly as the
  help orders them, and it falls back to GitHub when the cloud cache is off or
  fails.
* `memory_only_github_api`, `memory_no_disk_no_cloud`: the memory backend reads
  only the GitHub API and its code reaches neither the disk cache nor the cloud
  client.
* `names_match_help`, `cloud_limits_match_help`, `help_no_credentials`: every
  variable name, the cache directory, the download limits, and "carry no
  credentials" (`Wit.ReadOnly.client_sends_no_credentials`) match the code.

**Assumptions.** URL validation (`parse_base_url`) is a parameter `valid`; the
hosted URL being valid is a hypothesis, checked by the Rust test
`config_disable_values_and_url_rules`. A fill step either succeeds or fails;
what a successful cloud fill contains is `Wit.Integrity`'s subject. Lean's
`Char.toLower` is ASCII-only like Rust's `to_ascii_lowercase`, but
`Char.isWhitespace` covers only ASCII whitespace where Rust's `trim` covers
Unicode `White_Space`.
-/

namespace Wit.CacheSource

/-- A raw environment or flag value. -/
abbrev Key := List Char

/-- `str::trim` (ASCII whitespace). -/
def trim (cs : Key) : Key :=
  ((cs.dropWhile Char.isWhitespace).reverse.dropWhile Char.isWhitespace).reverse

/-- Trimmed and ASCII-lowercased, as `parse` and `is_disabled` compare values. -/
def norm (cs : Key) : Key := (trim cs).map Char.toLower

def memberOf (cs : Key) (values : List String) : Bool :=
  values.any fun v => decide (v.toList = norm cs)

/-! ## Backend: `CliSnapshotBackend::from_env_or_flag` -/

/-- `CliSnapshotBackend::parse`; `none` is its "unknown snapshot backend" error. -/
def parseBackend (cs : Key) : Option Backend :=
  if memberOf cs Src.diskBackendAliases then some .disk
  else if memberOf cs Src.memoryBackendAliases then some .memory
  else none

/-- `flag` is `--backend`, `env` is `WIT_SNAPSHOT_BACKEND`. -/
def resolveBackend (flag env : Option Key) : Option Backend :=
  match flag, env with
  | some v, _ => parseBackend v
  | none, some v => if trim v = [] then some Src.backendDefault else parseBackend v
  | none, none => some Src.backendDefault

theorem default_backend_matches_help :
    resolveBackend none none = some Src.helpDefaultBackend ∧
      resolveBackend none (some "".toList) = some Src.helpDefaultBackend ∧
      resolveBackend none (some " \t".toList) = some Src.helpDefaultBackend ∧
      Src.helpFlagDefaultBackends.all (· = Src.helpDefaultBackend) := by
  decide

theorem flag_wins_over_env (flag : Key) (env : Option Key) :
    resolveBackend (some flag) env = parseBackend flag := rfl

theorem documented_backends_parse :
    parseBackend "disk".toList = some .disk ∧ parseBackend "memory".toList = some .memory ∧
      parseBackend " DISK ".toList = some .disk ∧ parseBackend "Memory".toList = some .memory ∧
      resolveBackend none (some "memory".toList) = some .memory := by
  decide

/-! ## Shared cloud cache: `built_in_default` and `CloudCacheConfig::from_values` -/

inductive Profile where
  | debug
  | release

/-- `built_in_default`: a URL baked with `WIT_DEFAULT_CACHE_URL` wins, else the profile's. -/
def builtInDefault (baked : Option Key) : Profile → Option Key
  | .debug => baked <|> Src.debugDefaultUrl.map String.toList
  | .release => baked <|> Src.releaseDefaultUrl.map String.toList

def isDisabled (cs : Key) : Bool := memberOf cs Src.clientDisableValues

/-- The disable check, then URL validation (`parse_base_url`, abstract here). -/
def gate (valid : Key → Bool) (raw : Key) : Option Key :=
  if isDisabled raw then none else if valid raw then some raw else none

/-- The cloud cache base URL in effect; `none` is off. `env` is `WIT_CACHE_URL`. -/
def cloudUrl (valid : Key → Bool) (baked : Option Key) (p : Profile) : Option Key → Option Key
  | some raw => gate valid raw
  | none => (builtInDefault baked p).bind (gate valid)

theorem hosted_not_disabled : isDisabled Src.hostedCacheUrl.toList = false := by decide

theorem release_default_matches_help (valid : Key → Bool)
    (hvalid : valid Src.hostedCacheUrl.toList = true) :
    cloudUrl valid none .release none = Src.helpReleaseDefaultUrl.map String.toList := by
  have hcode : Src.releaseDefaultUrl = some Src.hostedCacheUrl := rfl
  have hhelp : Src.helpReleaseDefaultUrl = some Src.hostedCacheUrl := rfl
  rw [hhelp]
  simp [cloudUrl, builtInDefault, gate, hcode, hosted_not_disabled, hvalid]

theorem debug_default_matches_help (valid : Key → Bool) :
    cloudUrl valid none .debug none = Src.helpDebugDefaultUrl.map String.toList := by
  simp [cloudUrl, builtInDefault, Src.debugDefaultUrl, Src.helpDebugDefaultUrl]

theorem env_url_enables_any_build (valid : Key → Bool) (baked : Option Key) (p : Profile)
    (url : Key) (hvalid : valid url = true) (hon : isDisabled url = false) :
    cloudUrl valid baked p (some url) = some url := by
  simp [cloudUrl, gate, hvalid, hon]

theorem disable_values_match_help :
    (∀ v ∈ Src.helpDisableValues, v ∈ Src.clientDisableValues) ∧
      ∀ v ∈ Src.clientDisableValues, v ∈ Src.helpDisableValues := by
  decide

theorem help_disable_values_normalized :
    ∀ v ∈ Src.helpDisableValues,
      isDisabled v.toList = true ∧ isDisabled (v.toList.map Char.toUpper) = true := by
  decide

theorem help_disable_values_disable (valid : Key → Bool) (baked : Option Key) (p : Profile) :
    ∀ v ∈ Src.helpDisableValues,
      cloudUrl valid baked p (some v.toList) = none ∧
        cloudUrl valid baked p (some (v.toList.map Char.toUpper)) = none := by
  intro v hv
  obtain ⟨h, hu⟩ := help_disable_values_normalized v hv
  simp [cloudUrl, gate, h, hu]

/-! ## Disk reads: `cache_github_repo_target`, `recache_repo`, `refresh_repo_with_context` -/

structure World where
  /-- A usable local entry exists and the read does not force a refresh. -/
  warm : Bool
  /-- `cloudUrl` for this process. -/
  cloud : Option Key
  /-- The cloud cache serves a pack for the resolved commit that verifies. -/
  cloudVerifies : Bool
  /-- The GitHub clone (gix, then the git CLI) succeeds. -/
  githubOk : Bool

def succeeds (w : World) : Source → Bool
  | .localCache => w.warm
  | .cloud => w.cloud.isSome && w.cloudVerifies
  | .github => w.githubOk
  | .githubApi => false

def firstSuccess (w : World) : List Source → Option Source
  | [] => none
  | s :: rest => if succeeds w s then some s else firstSuccess w rest

def diskRead (w : World) : Option Source := firstSuccess w Src.diskReadOrder

theorem disk_read_order_matches_help : Src.diskReadOrder = Src.helpDiskReadOrder := by decide

theorem warm_cache_served_locally (w : World) (h : w.warm = true) :
    diskRead w = some .localCache := by
  simp [diskRead, Src.diskReadOrder, firstSuccess, succeeds, h]

theorem cloud_first_when_enabled (w : World) (hw : w.warm = false) (hc : w.cloud.isSome = true)
    (hv : w.cloudVerifies = true) : diskRead w = some .cloud := by
  simp [diskRead, Src.diskReadOrder, firstSuccess, succeeds, hw, hc, hv]

theorem falls_back_to_github (w : World) (hw : w.warm = false)
    (hc : w.cloud = none ∨ w.cloudVerifies = false) :
    diskRead w = if w.githubOk then some .github else none := by
  have : (w.cloud.isSome && w.cloudVerifies) = false := by
    rcases hc with h | h <;> simp [h]
  cases hg : w.githubOk <;>
    simp [diskRead, Src.diskReadOrder, firstSuccess, succeeds, hw, hg, this]

theorem cloud_only_when_enabled (w : World) (h : diskRead w = some .cloud) :
    w.cloud.isSome = true := by
  cases hw : w.warm <;> cases hc : w.cloud.isSome <;> cases hv : w.cloudVerifies <;>
    cases hg : w.githubOk <;>
    simp_all [diskRead, Src.diskReadOrder, firstSuccess, succeeds]

theorem debug_build_never_cloud (valid : Key → Bool) (w : World)
    (h : w.cloud = cloudUrl valid none .debug none) : diskRead w ≠ some .cloud := by
  intro hr
  have := cloud_only_when_enabled w hr
  rw [h, debug_default_matches_help] at this
  simp [Src.helpDebugDefaultUrl] at this

theorem release_build_tries_cloud (valid : Key → Bool)
    (hvalid : valid Src.hostedCacheUrl.toList = true) (w : World)
    (h : w.cloud = cloudUrl valid none .release none) (hw : w.warm = false)
    (hv : w.cloudVerifies = true) : diskRead w = some .cloud := by
  apply cloud_first_when_enabled w hw _ hv
  rw [h, release_default_matches_help valid hvalid]
  simp [Src.helpReleaseDefaultUrl]

theorem disk_never_github_api (w : World) : diskRead w ≠ some .githubApi := by
  cases hw : w.warm <;> cases hc : w.cloud.isSome <;> cases hv : w.cloudVerifies <;>
    cases hg : w.githubOk <;>
    simp_all [diskRead, Src.diskReadOrder, firstSuccess, succeeds]

/-! ## Memory reads -/

def read : Backend → World → Bool → Option Source
  | .disk, w, _ => diskRead w
  | .memory, _, apiOk => if apiOk then some .githubApi else none

theorem memory_only_github_api (w : World) (apiOk : Bool) :
    read .memory w apiOk = none ∨ read .memory w apiOk = some .githubApi := by
  cases apiOk <;> simp [read]

theorem memory_no_disk_no_cloud :
    Src.memoryUsesDiskCache = false ∧ Src.memoryUsesCloud = false ∧
      Src.helpMemoryCacheDirEnvVar = Src.cacheDirEnvVar := by
  decide

/-! ## Names and limits -/

theorem names_match_help :
    Src.helpBackendEnvVar = Src.backendEnvVar ∧
      Src.helpFlagEnvVars.all (· = Src.backendEnvVar) ∧
      Src.helpCloudUrlEnvVars.all (· = Src.cloudUrlEnvVar) ∧
      Src.helpBakedDefaultEnvVar = Src.bakedDefaultEnvVar ∧
      Src.helpCloudTimeoutEnvVar = Src.cloudTimeoutEnvVar ∧
      Src.helpCloudMaxBytesEnvVar = Src.cloudMaxBytesEnvVar ∧
      Src.helpCacheDirEnvVar = Src.cacheDirEnvVar ∧ Src.helpCacheSubdir = Src.cacheSubdir := by
  decide

theorem cloud_limits_match_help :
    Src.helpCloudTimeoutMs = Src.clientDefaultTimeoutMs ∧
      Src.helpCloudMaxBytes = Src.clientDefaultMaxBytes := by
  decide

theorem help_no_credentials :
    Src.clientMethods = ["GET"] ∧ "authorization" ∉ Src.clientHeaders ∧
      Src.clientHeaders ⊆ ["user-agent"] ∧ Src.clientRedirect = "none" :=
  ReadOnly.client_sends_no_credentials

end Wit.CacheSource
