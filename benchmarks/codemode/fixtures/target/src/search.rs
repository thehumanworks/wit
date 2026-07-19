pub struct Match {
    pub path: &'static str,
    pub score: u32,
}

pub fn rank_matches(mut matches: Vec<Match>) -> Vec<Match> {
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(right.path))
    });
    matches
}

pub fn keep_rust_sources(matches: Vec<Match>) -> Vec<Match> {
    matches
        .into_iter()
        .filter(|candidate| candidate.path.ends_with(".rs"))
        .collect()
}
