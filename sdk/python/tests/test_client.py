import json
import unittest
from typing import Dict, List, Tuple
from urllib.parse import parse_qs, urlparse

from wit_api import WitClient, WitError, build_query

PROV = {
    "api_version": "2",
    "repo": "o/r",
    "requested_ref": "main",
    "ref": "refs/heads/main",
    "commit": "c" * 40,
    "cache": "miss",
}


class FakeTransport:
    """Route table keyed by path; records every request."""

    def __init__(self, routes):
        self.routes = routes
        self.calls: List[Tuple[str, Dict[str, str]]] = []

    def __call__(self, url, headers):
        self.calls.append((url, dict(headers)))
        parsed = urlparse(url)
        handler = self.routes.get(parsed.path) or self.routes.get("*")
        if handler is None:
            return 404, {}, b"error: not found\n"
        return handler(parsed)


def ok_json(payload, status=200, headers=None):
    return status, {"content-type": "application/json", **(headers or {})}, json.dumps(payload).encode()


class BuildQueryTest(unittest.TestCase):
    def test_drops_empty_values_expands_lists_and_maps_booleans(self):
        self.assertEqual(
            build_query({"path": "src", "l": True, "v": False, "n": None, "x": "", "ignore": ["a", "b"], "max": 5}),
            "path=src&l=1&ignore=a&ignore=b&max=5",
        )


class WitClientTest(unittest.TestCase):
    def test_defaults_headers_and_url(self):
        transport = FakeTransport({"*": lambda p: ok_json({**PROV, "verb": "ls", "path": ".", "entries": []})})
        client = WitClient(token="ghp_x", headers={"X-Test": "1"}, transport=transport)
        client.repo("o/r").ls()
        url, headers = transport.calls[0]
        self.assertEqual(url, "https://wit.thehuman.sh/api/ls/o/r?l=1&format=json")
        self.assertEqual(headers["Authorization"], "Bearer ghp_x")
        self.assertEqual(headers["Accept"], "application/json")
        self.assertEqual(headers["X-Test"], "1")
        self.assertTrue(headers["User-Agent"].startswith("wit-api-python/"))

    def test_every_verb_builds_the_expected_url(self):
        transport = FakeTransport({"*": lambda p: ok_json(dict(PROV))})
        repo = WitClient("http://h/api/", transport=transport).repo("o/r", "dev")
        repo.stats("src", largest=3, fresh=True, ignore=["*.md"])
        repo.tree("src", depth=1)
        repo.cat("a.rs", lines=(10, 20))
        repo.cat("a.rs", lines=(None, 5))
        repo.cat("a.rs", lines="7-")
        repo.head("a.rs", 3)
        repo.tail("a.rs", plus=40)
        repo.outline("a.rs", max_symbols=10)
        repo.rg("fn main", glob="*.rs", ignore_case=True, context=2, max_matches=9, max_files=4)
        repo.rg_files("todo", path="src", long=True)
        repo.rg_counts("x", before=1, after=2)
        repo.refs()
        repo.commits("a.rs", n=3)
        repo.at(None).ls("src")
        paths = [url.replace("http://h/api", "") for url, _ in transport.calls]
        self.assertEqual(
            paths,
            [
                "/stats/o/r?ref=dev&fresh=1&ignore=%2A.md&path=src&largest=3&format=json",
                "/tree/o/r?ref=dev&path=src&depth=1&l=1&format=json",
                "/cat/o/r?ref=dev&path=a.rs&lines=10-20&format=json",
                "/cat/o/r?ref=dev&path=a.rs&lines=-5&format=json",
                "/cat/o/r?ref=dev&path=a.rs&lines=7-&format=json",
                "/head/o/r?ref=dev&path=a.rs&lines=3&format=json",
                "/tail/o/r?ref=dev&path=a.rs&plus=40&format=json",
                "/outline/o/r?ref=dev&path=a.rs&max_symbols=10&format=json",
                "/rg/o/r?ref=dev&q=fn+main&glob=%2A.rs&i=1&C=2&max=9&max_files=4&format=json",
                "/rg/o/r?ref=dev&q=todo&path=src&long=1&l=1&format=json",
                "/rg/o/r?ref=dev&q=x&B=1&A=2&c=1&format=json",
                "/refs/o/r?format=json",
                "/commits/o/r?ref=dev&path=a.rs&n=3&format=json",
                "/ls/o/r?path=src&l=1&format=json",
            ],
        )

    def test_search_and_text(self):
        transport = FakeTransport(
            {
                "/api/search": lambda p: ok_json({"api_version": "2", "verb": "search", "query": "x", "sort": "stars", "total_count": 0, "items": []}),
                "/api/tree/o/r": lambda p: (200, {"content-type": "text/plain"}, b"src\n  lib.rs\n"),
            }
        )
        client = WitClient("http://h/api", transport=transport)
        result = client.search("terminal ui", lang="rust", limit=5, sort="best")
        self.assertEqual(result["total_count"], 0)
        self.assertEqual(transport.calls[0][0], "http://h/api/search?q=terminal+ui&lang=rust&limit=5&sort=best&format=json")
        text = client.repo("o/r").text("tree", path="src")
        self.assertEqual(text, "src\n  lib.rs\n")
        self.assertEqual(transport.calls[1][0], "http://h/api/tree/o/r?path=src")
        self.assertEqual(transport.calls[1][1]["Accept"], "text/plain")

    def test_errors_carry_code_status_and_retry_after(self):
        transport = FakeTransport(
            {
                "/api/cat/o/r": lambda p: ok_json({"error": "File not found: x", "code": "not_found", "status": 404}, status=404),
                "/api/tree/o/r": lambda p: (429, {"retry-after": "12"}, b"error: GitHub API rate limit exceeded (resets in 12s). hint\n"),
            }
        )
        client = WitClient("http://h/api", transport=transport)
        with self.assertRaises(WitError) as ctx:
            client.repo("o/r").cat("x")
        self.assertEqual(ctx.exception.status, 404)
        self.assertEqual(ctx.exception.code, "not_found")
        self.assertEqual(str(ctx.exception), "File not found: x")
        self.assertFalse(ctx.exception.is_rate_limited)
        with self.assertRaises(WitError) as ctx:
            client.repo("o/r").text("tree")
        self.assertEqual(ctx.exception.status, 429)
        self.assertEqual(ctx.exception.retry_after, 12.0)
        self.assertTrue(ctx.exception.is_rate_limited)
        self.assertTrue(str(ctx.exception).startswith("GitHub API rate limit exceeded"))

    def test_retries_429_honouring_retry_after(self):
        attempts = {"n": 0}

        def flaky(p):
            attempts["n"] += 1
            if attempts["n"] < 3:
                return 429, {"retry-after": "60"}, b"error: slow\n"
            return ok_json({**PROV, "verb": "ls", "path": ".", "entries": []})

        transport = FakeTransport({"/api/ls/o/r": flaky})
        slept = []
        client = WitClient("http://h/api", transport=transport, retries=2, max_retry_delay=0.01, sleep=slept.append)
        self.assertEqual(client.repo("o/r").ls()["verb"], "ls")
        self.assertEqual(attempts["n"], 3)
        self.assertEqual(slept, [0.01, 0.01])

        attempts["n"] = 0
        strict = WitClient("http://h/api", transport=transport)
        with self.assertRaises(WitError):
            strict.repo("o/r").ls()
        self.assertEqual(attempts["n"], 1)

    def test_read_symbol_chains_outline_and_cat(self):
        def cat(p):
            start, end = parse_qs(p.query)["lines"][0].split("-")
            return ok_json({**PROV, "verb": "cat", "path": "a.rs", "blob_sha": "b", "total_lines": 40, "start_line": int(start), "end_line": int(end), "text": "code"})

        transport = FakeTransport(
            {
                "/api/outline/o/r": lambda p: ok_json(
                    {
                        **PROV,
                        "verb": "outline",
                        "path": "a.rs",
                        "blob_sha": "b",
                        "language": "Rust",
                        "supported": True,
                        "total_lines": 40,
                        "truncated": False,
                        "symbols": [
                            {"line": 3, "end_line": 9, "kind": "struct", "name": "Widget", "signature": "pub struct Widget {"},
                            {"line": 12, "end_line": 30, "kind": "impl", "name": "Widget", "signature": "impl Widget {"},
                        ],
                    }
                ),
                "/api/cat/o/r": cat,
            }
        )
        repo = WitClient("http://h/api", transport=transport).repo("o/r")
        hit = repo.read_symbol("a.rs", "Widget", kind="impl", padding=2)
        self.assertEqual(hit["symbol"]["kind"], "impl")
        self.assertEqual((hit["start_line"], hit["end_line"]), (10, 32))
        self.assertIn("lines=10-32", transport.calls[1][0])
        self.assertIsNone(repo.read_symbol("a.rs", "nope"))

    def test_context_locates_then_reads_windows(self):
        transport = FakeTransport(
            {
                "/api/rg/o/r": lambda p: ok_json(
                    {
                        **PROV,
                        "verb": "rg",
                        "pattern": "x",
                        "path": ".",
                        "glob": None,
                        "files_scanned": 2,
                        "files_candidate": 2,
                        "files_skipped_binary": 0,
                        "match_count": 3,
                        "truncated": False,
                        "matches": [
                            {"path": "a.rs", "line": 5, "text": "x", "is_context": False},
                            {"path": "a.rs", "line": 9, "text": "x", "is_context": False},
                            {"path": "b.rs", "line": 100, "text": "x", "is_context": False},
                        ],
                    }
                ),
                "/api/cat/o/r": lambda p: ok_json({**PROV, "verb": "cat", "path": parse_qs(p.query)["path"][0], "blob_sha": "b", "total_lines": 200, "start_line": 1, "end_line": 2, "text": "snippet"}),
            }
        )
        repo = WitClient("http://h/api", transport=transport).repo("o/r")
        snippets = repo.context("x", window=3, max_snippets=5)
        self.assertEqual([s["path"] for s in snippets], ["a.rs", "b.rs"])
        self.assertEqual(snippets[0]["commit"], PROV["commit"])
        self.assertIn("path=a.rs&lines=2-8", transport.calls[1][0])
        self.assertIn("path=b.rs&lines=97-103", transport.calls[2][0])
        self.assertIn("max=20", transport.calls[0][0])

    def test_rejects_malformed_owner_repo(self):
        client = WitClient(transport=lambda url, headers: (200, {}, b""))
        with self.assertRaises(ValueError):
            client.repo("nope")
        with self.assertRaises(ValueError):
            client.repo("a/b/c")


if __name__ == "__main__":
    unittest.main()
