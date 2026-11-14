//! Compact text rendering of direct MCP tool results.
//!
//! Direct tools return the full response as `structuredContent`. Most agent hosts show the model
//! the text `content` block instead, and repeating per-item provenance (repo, commit, blob,
//! snapshot) on every line costs agents several times more tokens than the source text itself.
//! The text block therefore states shared provenance once and prints lines as `N: text`.
//! Set `WIT_MCP_CONTENT=json` to get the serialized structured response as text instead.

use serde_json::{Map, Value};

/// Fields that never help a model read the result.
const DROPPED: &[&str] = &[
    "api_version",
    "budget",
    "blob_sha",
    "format",
    "returned_items",
    "returned_lines",
];
/// Shared fields printed once in the header, in this order.
const HEADER: &[&str] = &[
    "repo",
    "requested_ref",
    "resolved_ref",
    "commit_sha",
    "snapshot_id",
    "path",
];

pub fn json_content_requested() -> bool {
    std::env::var("WIT_MCP_CONTENT").is_ok_and(|value| value.eq_ignore_ascii_case("json"))
}

pub fn agent_text(value: &Value) -> String {
    let Value::Object(top) = value else {
        return value.to_string();
    };
    let mut shared = top.clone();
    let items = match shared.remove("items") {
        Some(Value::Array(items)) => Some(items),
        Some(other) => {
            shared.insert("items".into(), other);
            None
        }
        None => None,
    };
    let mut items: Vec<Map<String, Value>> = items
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| match item {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .collect();
    // Hoist provenance that every item shares into the header.
    if let Some(first) = items.first().cloned() {
        for key in HEADER {
            if let Some(value) = first.get(*key)
                && !shared.contains_key(*key)
                && items.iter().all(|item| item.get(*key) == Some(value))
            {
                shared.insert((*key).into(), value.clone());
            }
        }
        for item in &mut items {
            for key in HEADER {
                if shared.contains_key(*key) {
                    item.remove(*key);
                }
            }
        }
        // Any other scalar every item repeats (kind, language, query) is printed once too.
        if items.len() > 1 {
            let repeated: Vec<(String, Value)> = items[0]
                .iter()
                .filter(|(key, value)| {
                    !matches!(value, Value::Array(_) | Value::Object(_))
                        && !matches!(key.as_str(), "start_line" | "end_line" | "text")
                        && items.iter().all(|item| item.get(*key) == Some(*value))
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            for (key, value) in repeated {
                for item in &mut items {
                    item.remove(&key);
                }
                shared.entry(key).or_insert(value);
            }
        }
    }

    let mut out = Vec::new();
    let header: Vec<String> = HEADER
        .iter()
        .filter_map(|key| scalar(shared.get(*key)?).map(|v| format!("{key}: {v}")))
        .collect();
    out.extend(header);
    for key in HEADER {
        shared.remove(*key);
    }
    let has_more = shared.remove("has_more");
    let next_cursor = shared.remove("next_cursor");

    // wit_read format=lines, and line arrays in general.
    if let Some(Value::Array(lines)) = shared.remove("lines") {
        out.push(String::new());
        out.extend(render_lines(&lines));
    }
    // wit_read format=text: number the lines so they can be cited.
    if let Some(Value::String(text)) = shared.remove("text") {
        let start = shared
            .get("start_line")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        out.push(String::new());
        out.extend(
            text.split('\n')
                .enumerate()
                .map(|(i, line)| format!("{}: {line}", start + i as u64)),
        );
    }
    if let Some(Value::Array(paths)) = shared.remove("paths") {
        out.push(String::new());
        out.extend(paths.iter().filter_map(scalar));
    }
    if let Some(Value::String(rendered)) = shared.remove("rendered_text") {
        out.push(String::new());
        out.push(rendered);
    }
    // wit_read structured: one item per line.
    let line_items = !items.is_empty()
        && items.iter().all(|item| {
            item.contains_key("text")
                && item.contains_key("start_line")
                && !item.contains_key("lines")
        });
    if line_items {
        out.push(String::new());
        for item in &items {
            let number = item.get("start_line").and_then(scalar).unwrap_or_default();
            let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
            out.push(format!("{number}: {text}"));
        }
    } else if !items.is_empty() {
        let items = coalesce_snippets(items.clone());
        let mut previous_had_lines = true;
        for item in &items {
            let has_lines = item.get("lines").is_some_and(Value::is_array);
            // Blank lines separate snippets and the header; one-line items stay on adjacent lines.
            if has_lines || previous_had_lines {
                out.push(String::new());
            }
            previous_had_lines = has_lines;
            out.extend(render_item(item));
        }
    }
    let rest = fields(&shared);
    if !rest.is_empty() {
        out.insert(header_len(&out), rest);
    }
    match (
        has_more.as_ref().and_then(Value::as_bool),
        next_cursor.as_ref().and_then(Value::as_str),
    ) {
        (Some(true), Some(cursor)) => {
            out.push(String::new());
            out.push(format!(
                "has_more: true; pass next_cursor to continue: {cursor}"
            ));
        }
        (Some(true), None) => {
            out.push(String::new());
            out.push("has_more: true".into());
        }
        _ => {}
    }
    if items.is_empty() && top.get("items").is_some() {
        out.push(String::new());
        out.push("(no results)".into());
    }
    out.join("\n").trim().to_string()
}

/// Merge consecutive snippets of one file whose line windows overlap or touch, so nearby matches
/// print their shared context once. The merged item lists every match line in `match_lines`.
fn coalesce_snippets(items: Vec<Map<String, Value>>) -> Vec<Map<String, Value>> {
    let line_range = |item: &Map<String, Value>| {
        Some((
            item.get("start_line")?.as_u64()?,
            item.get("end_line")?.as_u64()?,
        ))
    };
    let mut out: Vec<Map<String, Value>> = Vec::with_capacity(items.len());
    for mut item in items {
        let mergeable = match (out.last(), line_range(&item)) {
            (Some(previous), Some((start, _))) => {
                previous.get("lines").is_some_and(Value::is_array)
                    && item.get("lines").is_some_and(Value::is_array)
                    && previous.get("path") == item.get("path")
                    && line_range(previous).is_some_and(|(_, end)| start <= end + 1)
            }
            _ => false,
        };
        if !mergeable {
            out.push(item);
            continue;
        }
        let previous = out.last_mut().expect("checked above");
        let mut matches: Vec<String> = match previous.remove("match_lines") {
            Some(Value::String(list)) => list.split(',').map(str::to_string).collect(),
            _ => previous
                .remove("match_line")
                .and_then(|v| scalar(&v))
                .into_iter()
                .collect(),
        };
        if let Some(line) = item.remove("match_line").and_then(|v| scalar(&v))
            && !matches.contains(&line)
        {
            matches.push(line);
        }
        previous.insert("match_lines".into(), Value::String(matches.join(",")));
        let (Some(Value::Array(lines)), Some(Value::Array(more))) =
            (previous.get_mut("lines"), item.remove("lines"))
        else {
            continue;
        };
        let last = lines
            .last()
            .and_then(|line| line.get("line_number")?.as_u64())
            .unwrap_or(0);
        lines.extend(
            more.into_iter()
                .filter(|line| line.get("line_number").and_then(Value::as_u64) > Some(last)),
        );
        if let Some(end) = item.get("end_line").cloned() {
            let longer = line_range(&item).map(|(_, e)| e) > line_range(previous).map(|(_, e)| e);
            if longer {
                previous.insert("end_line".into(), end);
            }
        }
        if previous.get("query") != item.get("query")
            && let (Some(Value::String(a)), Some(Value::String(b))) =
                (previous.get("query").cloned(), item.get("query"))
        {
            previous.insert("query".into(), Value::String(format!("{a} | {b}")));
        }
    }
    out
}

fn header_len(out: &[String]) -> usize {
    out.iter().take_while(|line| !line.is_empty()).count()
}

fn render_item(item: &Map<String, Value>) -> Vec<String> {
    let mut item = item.clone();
    let Some(Value::Array(lines)) = item.remove("lines") else {
        return vec![fields(&item)];
    };
    // The numbered lines carry the range, so only the path stays in the item header.
    let start = item.remove("start_line");
    let end = item.remove("end_line");
    let mut head = Vec::new();
    if let Some(Value::String(path)) = item.remove("path") {
        let start = start.as_ref().and_then(scalar).unwrap_or_default();
        let end = end.as_ref().and_then(scalar).unwrap_or_default();
        head.push(if start == end {
            format!("{path}:{start}")
        } else {
            format!("{path}:{start}-{end}")
        });
    }
    let rest = fields(&item);
    if !rest.is_empty() {
        head.push(rest);
    }
    let mut out = vec![head.join("  ")];
    out.extend(render_lines(&lines).into_iter().map(clip_snippet_line));
    out
}

/// Search and context snippets are for finding code, not reading it, and generated files (API
/// specs, vendored bundles) put tens of kilobytes on one line. A long line is cut, and the cut is
/// marked so the agent can read the full line with wit_read.
const MAX_SNIPPET_LINE_CHARS: usize = 300;

fn clip_snippet_line(line: String) -> String {
    let total = line.chars().count();
    if total <= MAX_SNIPPET_LINE_CHARS {
        return line;
    }
    let kept: String = line.chars().take(MAX_SNIPPET_LINE_CHARS).collect();
    format!(
        "{kept} …[+{} chars; wit_read shows the full line]",
        total - MAX_SNIPPET_LINE_CHARS
    )
}

fn render_lines(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let number = line.get("line_number").and_then(scalar).unwrap_or_default();
            let text = line.get("text").and_then(Value::as_str).unwrap_or_default();
            format!("{number}: {text}")
        })
        .collect()
}

/// `key: value` pairs on one line, skipping nulls and fields that never help a model.
fn fields(map: &Map<String, Value>) -> String {
    map.iter()
        .filter(|(key, value)| !value.is_null() && !DROPPED.contains(&key.as_str()))
        .filter(|(_, value)| !matches!(value, Value::Array(a) if a.is_empty()))
        .map(|(key, value)| {
            let value = scalar(value).unwrap_or_else(|| value.to_string());
            format!("{key}={value}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::agent_text;
    use serde_json::json;

    #[test]
    fn search_hoists_provenance_and_numbers_lines() {
        let value = json!({
            "api_version": "2", "budget": {"serialized_bytes": 1}, "has_more": false, "next_cursor": null,
            "returned_items": 1,
            "items": [{
                "blob_sha": "b", "commit_sha": "c", "repo": "o/r", "snapshot_id": "o/r@c",
                "path": "src/a.rs", "start_line": 1, "end_line": 2, "match_line": 2, "query": "x",
                "lines": [{"line_number": 1, "text": "fn a() {"}, {"line_number": 2, "text": "  x"}]
            }]
        });
        assert_eq!(
            agent_text(&value),
            "repo: o/r\ncommit_sha: c\nsnapshot_id: o/r@c\npath: src/a.rs\n\nmatch_line=2 query=x\n1: fn a() {\n2:   x"
        );
    }

    #[test]
    fn multi_file_search_keeps_paths_on_items() {
        let line = |n: u64| json!([{"line_number": n, "text": "t"}]);
        let value = json!({"has_more": true, "next_cursor": "abc", "items": [
            {"repo": "o/r", "path": "a.rs", "start_line": 3, "end_line": 3, "lines": line(3)},
            {"repo": "o/r", "path": "b.rs", "start_line": 9, "end_line": 9, "lines": line(9)}
        ]});
        assert_eq!(
            agent_text(&value),
            "repo: o/r\n\na.rs:3\n3: t\n\nb.rs:9\n9: t\n\nhas_more: true; pass next_cursor to continue: abc"
        );
    }

    #[test]
    fn read_formats_print_numbered_lines() {
        let text = json!({"repo": "o/r", "path": "a.rs", "start_line": 10, "end_line": 11, "format": "text", "text": "a\nb"});
        assert_eq!(
            agent_text(&text),
            "repo: o/r\npath: a.rs\nend_line=11 start_line=10\n\n10: a\n11: b"
        );
        let structured = json!({"items": [
            {"repo": "o/r", "path": "a.rs", "start_line": 4, "end_line": 4, "text": "x"},
            {"repo": "o/r", "path": "a.rs", "start_line": 5, "end_line": 5, "text": "y"}
        ]});
        assert_eq!(
            agent_text(&structured),
            "repo: o/r\npath: a.rs\n\n4: x\n5: y"
        );
    }

    #[test]
    fn listings_print_one_entry_per_line() {
        let value = json!({"items": [
            {"repo": "o/r", "kind": "file", "path": "a.rs", "lines": null},
            {"repo": "o/r", "kind": "file", "path": "b.rs", "lines": null}
        ]});
        assert_eq!(
            agent_text(&value),
            "repo: o/r\nkind=file\n\npath=a.rs\npath=b.rs"
        );
    }

    #[test]
    fn overlapping_matches_in_one_file_share_context() {
        let lines = |range: std::ops::RangeInclusive<u64>| {
            range
                .map(|n| json!({"line_number": n, "text": format!("l{n}")}))
                .collect::<Vec<_>>()
        };
        let value = json!({"items": [
            {"repo": "o/r", "path": "a.rs", "start_line": 1, "end_line": 5, "match_line": 3, "query": "x", "lines": lines(1..=5)},
            {"repo": "o/r", "path": "a.rs", "start_line": 4, "end_line": 8, "match_line": 6, "query": "x", "lines": lines(4..=8)},
            {"repo": "o/r", "path": "a.rs", "start_line": 20, "end_line": 22, "match_line": 21, "query": "x", "lines": lines(20..=22)}
        ]});
        assert_eq!(
            agent_text(&value),
            "repo: o/r\npath: a.rs\nquery=x\n\nmatch_lines=3,6\n1: l1\n2: l2\n3: l3\n4: l4\n5: l5\n6: l6\n7: l7\n8: l8\n\nmatch_line=21\n20: l20\n21: l21\n22: l22"
        );
    }

    #[test]
    fn long_snippet_lines_are_clipped() {
        let long = "x".repeat(350);
        let value = json!({"items": [{"path": "a.json", "start_line": 1, "end_line": 1,
            "lines": [{"line_number": 1, "text": long}]}]});
        let text = agent_text(&value);
        assert!(
            text.ends_with(&format!(
                "1: {} …[+53 chars; wit_read shows the full line]",
                "x".repeat(297)
            )),
            "{text}"
        );
    }

    #[test]
    fn empty_results_say_so() {
        assert_eq!(
            agent_text(&json!({"items": [], "has_more": false})),
            "(no results)"
        );
    }
}
