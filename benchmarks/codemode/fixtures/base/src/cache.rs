pub const DEFAULT_TTL_SECONDS: u64 = 300;

pub fn is_fresh(age_seconds: u64) -> bool {
    age_seconds < DEFAULT_TTL_SECONDS
}

pub fn cache_state(age_seconds: u64) -> &'static str {
    if is_fresh(age_seconds) {
        "fresh"
    } else {
        "stale"
    }
}
