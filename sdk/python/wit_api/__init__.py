"""wit URL API client — zero-dependency Python SDK for https://wit.thehuman.sh/api.

Every method maps to one ``GET`` with ``?format=json`` and returns the decoded
body (which carries provenance: ``repo``, ``ref``, ``commit``, ``cache``).
Failures raise :class:`WitError` with the API's ``code`` and ``status``.

Chaining example (the workflow an agent should follow)::

    from wit_api import WitClient

    wit = WitClient()
    repo = wit.repo("ratatui/ratatui")
    stats = repo.stats()                                   # size / tokens / languages
    hits = repo.rg_files("impl Widget for", glob="*.rs")
    outline = repo.outline(hits["files"][0]["path"])
    impl = next(s for s in outline["symbols"] if s["kind"] == "impl")
    code = repo.cat(hits["files"][0]["path"], lines=(impl["line"], impl["end_line"]))
    print(code["commit"], code["text"])
"""

from __future__ import annotations

import json
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Callable, Dict, Iterable, List, Mapping, Optional, Sequence, Tuple, Union

__all__ = ["DEFAULT_BASE_URL", "WitClient", "WitError", "WitRepo", "build_query"]
__version__ = "0.1.0"

DEFAULT_BASE_URL = "https://wit.thehuman.sh/api"

#: ``(status, headers, body)`` as returned by a transport.
Response = Tuple[int, Mapping[str, str], bytes]
#: ``transport(url, headers) -> (status, headers, body)``; must not raise on HTTP errors.
Transport = Callable[[str, Mapping[str, str]], Response]

Query = Mapping[str, Union[str, int, bool, Sequence[str], None]]
Lines = Union[str, Tuple[Optional[int], Optional[int]], None]


class WitError(Exception):
    """Non-2xx response from the API, carrying its ``code``/``status``."""

    def __init__(self, message: str, *, status: int, code: str, url: str, retry_after: Optional[float] = None):
        super().__init__(message)
        self.status = status
        self.code = code
        self.url = url
        self.retry_after = retry_after

    @property
    def is_rate_limited(self) -> bool:
        """True for 429s: the GitHub quota behind the host (or your token) is exhausted."""
        return self.status == 429

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"WitError(status={self.status}, code={self.code!r}, message={str(self)!r})"


def build_query(params: Query) -> str:
    """Encode params, dropping ``None``/``False``/empty values; ``True`` becomes ``1``."""
    pairs: List[Tuple[str, str]] = []
    for key, value in params.items():
        if value is None or value is False or value == "":
            continue
        if isinstance(value, bool):
            pairs.append((key, "1"))
        elif isinstance(value, (list, tuple)):
            pairs.extend((key, str(v)) for v in value if v not in (None, ""))
        else:
            pairs.append((key, str(value)))
    return urllib.parse.urlencode(pairs)


def _urllib_transport(url: str, headers: Mapping[str, str]) -> Response:
    request = urllib.request.Request(url, headers=dict(headers), method="GET")
    try:
        with urllib.request.urlopen(request, timeout=60) as res:  # noqa: S310 - https only
            return res.status, {k.lower(): v for k, v in res.headers.items()}, res.read()
    except urllib.error.HTTPError as err:
        body = err.read() if hasattr(err, "read") else b""
        return err.code, {k.lower(): v for k, v in (err.headers or {}).items()}, body


def _lines_param(lines: Lines) -> Optional[str]:
    if lines is None:
        return None
    if isinstance(lines, str):
        return lines
    start, end = lines
    if start is None and end is None:
        return None
    return f"{'' if start is None else start}-{'' if end is None else end}"


class WitClient:
    """Entry point: bind repositories with :meth:`repo`, discover them with :meth:`search`."""

    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        *,
        token: Optional[str] = None,
        headers: Optional[Mapping[str, str]] = None,
        transport: Optional[Transport] = None,
        retries: int = 0,
        max_retry_delay: float = 30.0,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.headers = dict(headers or {})
        self.transport = transport or _urllib_transport
        self.retries = max(0, retries)
        self.max_retry_delay = max_retry_delay
        self._sleep = sleep

    def repo(self, owner_repo: str, ref: Optional[str] = None) -> "WitRepo":
        """Bind a repository (and optional branch / tag / commit SHA) for chained calls."""
        return WitRepo(self, owner_repo, ref)

    def search(
        self,
        query: Optional[str] = None,
        *,
        pattern: Optional[str] = None,
        lang: Optional[str] = None,
        limit: Optional[int] = None,
        sort: Optional[str] = None,
    ) -> Dict[str, Any]:
        """GitHub repository search: find ``owner/repo`` for "libraries that do X"."""
        return self.get_json("/search", {"q": query, "p": pattern, "lang": lang, "limit": limit, "sort": sort})

    def llms_text(self) -> str:
        """The agent guide served at ``/llms.txt``."""
        return self.get_text("/llms.txt", {})

    # -- low level -----------------------------------------------------------------

    def get_json(self, path: str, params: Query) -> Dict[str, Any]:
        """One verb as JSON."""
        merged: Dict[str, Any] = dict(params)
        merged["format"] = "json"
        _status, _headers, body = self._request(path, merged, "application/json")
        return json.loads(body.decode("utf-8"))

    def get_text(self, path: str, params: Query) -> str:
        """One verb as the CLI-identical plaintext."""
        _status, _headers, body = self._request(path, params, "text/plain")
        return body.decode("utf-8")

    def _request(self, path: str, params: Query, accept: str) -> Response:
        query = build_query(params)
        url = f"{self.base_url}{path if path.startswith('/') else '/' + path}"
        if query:
            url = f"{url}?{query}"
        headers = {"Accept": accept, "User-Agent": f"wit-api-python/{__version__}", **self.headers}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"

        attempt = 0
        while True:
            status, res_headers, body = self.transport(url, headers)
            if 200 <= status < 300:
                return status, res_headers, body
            err = self._to_error(status, res_headers, body, url)
            if err.status == 429 and attempt < self.retries:
                attempt += 1
                self._sleep(min(err.retry_after or 1.0, self.max_retry_delay))
                continue
            raise err

    @staticmethod
    def _to_error(status: int, headers: Mapping[str, str], body: bytes, url: str) -> WitError:
        text = body.decode("utf-8", errors="replace").strip()
        message, code = text, "error"
        try:
            parsed = json.loads(text)
            if isinstance(parsed, dict) and isinstance(parsed.get("error"), str):
                message = parsed["error"]
                code = str(parsed.get("code") or code)
        except ValueError:
            if message.startswith("error:"):
                message = message[len("error:"):].strip()
        retry_after: Optional[float] = None
        raw_retry = headers.get("retry-after")
        if raw_retry:
            try:
                retry_after = float(raw_retry)
            except ValueError:
                retry_after = None
        return WitError(message or f"HTTP {status}", status=status, code=code, url=url, retry_after=retry_after)


class WitRepo:
    """A repository bound to a client and (optionally) a ref. Every method is one request."""

    def __init__(self, client: WitClient, owner_repo: str, ref: Optional[str] = None) -> None:
        owner, sep, name = owner_repo.partition("/")
        if not sep or not owner or not name or "/" in name:
            raise ValueError(f"expected owner/repo, got {owner_repo!r}")
        self.client = client
        self.owner_repo = owner_repo
        self.ref = ref

    def at(self, ref: Optional[str]) -> "WitRepo":
        """Same repository at another branch, tag, or commit SHA."""
        return WitRepo(self.client, self.owner_repo, ref)

    def _verb(self, verb: str, params: Query, *, fresh: bool = False, ignore: Optional[Iterable[str]] = None) -> Dict[str, Any]:
        merged: Dict[str, Any] = {"ref": self.ref, "fresh": fresh, "ignore": list(ignore) if ignore else None}
        merged.update(params)
        return self.client.get_json(f"/{verb}/{self.owner_repo}", merged)

    def text(self, verb: str, **params: Any) -> str:
        """CLI-identical plaintext for any verb, e.g. ``text("tree", path="src", l=1)``."""
        merged: Dict[str, Any] = {"ref": self.ref}
        merged.update(params)
        return self.client.get_text(f"/{verb}/{self.owner_repo}", merged)

    # -- orientation ---------------------------------------------------------------

    def stats(self, path: Optional[str] = None, *, largest: Optional[int] = None, fresh: bool = False, ignore: Optional[Iterable[str]] = None) -> Dict[str, Any]:
        """Size, token estimate, language and directory breakdown — no blob reads."""
        return self._verb("stats", {"path": path, "largest": largest}, fresh=fresh, ignore=ignore)

    def tree(self, path: Optional[str] = None, *, depth: Optional[int] = None, fresh: bool = False, ignore: Optional[Iterable[str]] = None) -> Dict[str, Any]:
        return self._verb("tree", {"path": path, "depth": depth, "l": True}, fresh=fresh, ignore=ignore)

    def ls(self, path: Optional[str] = None, *, fresh: bool = False, ignore: Optional[Iterable[str]] = None) -> Dict[str, Any]:
        return self._verb("ls", {"path": path, "l": True}, fresh=fresh, ignore=ignore)

    def refs(self) -> Dict[str, Any]:
        return self.client.get_json(f"/refs/{self.owner_repo}", {})

    def commits(self, path: Optional[str] = None, *, n: Optional[int] = None) -> Dict[str, Any]:
        return self.client.get_json(f"/commits/{self.owner_repo}", {"ref": self.ref, "path": path, "n": n})

    # -- reading -------------------------------------------------------------------

    def cat(self, path: str, *, lines: Lines = None, fresh: bool = False) -> Dict[str, Any]:
        """File text; ``lines=(start, end)`` reads a one-based inclusive range."""
        return self._verb("cat", {"path": path, "lines": _lines_param(lines)}, fresh=fresh)

    def head(self, path: str, lines: Optional[int] = None, *, fresh: bool = False) -> Dict[str, Any]:
        return self._verb("head", {"path": path, "lines": lines}, fresh=fresh)

    def tail(self, path: str, lines: Optional[int] = None, *, plus: Optional[int] = None, fresh: bool = False) -> Dict[str, Any]:
        return self._verb("tail", {"path": path, "lines": lines, "plus": plus}, fresh=fresh)

    def outline(self, path: str, *, max_symbols: Optional[int] = None, fresh: bool = False) -> Dict[str, Any]:
        """Line-numbered symbol index for one file (regex heuristic, no AST)."""
        return self._verb("outline", {"path": path, "max_symbols": max_symbols}, fresh=fresh)

    # -- searching -----------------------------------------------------------------

    @staticmethod
    def _rg_params(
        pattern: str,
        path: Optional[str],
        glob: Optional[str],
        ignore_case: bool,
        smart_case: bool,
        word: bool,
        invert: bool,
        context: Optional[int],
        before: Optional[int],
        after: Optional[int],
        max_matches: Optional[int],
        max_files: Optional[int],
        long: bool,
    ) -> Dict[str, Any]:
        return {
            "q": pattern,
            "path": path,
            "glob": glob,
            "i": ignore_case,
            "S": smart_case,
            "w": word,
            "v": invert,
            "C": context,
            "B": before,
            "A": after,
            "max": max_matches,
            "max_files": max_files,
            "long": long,
        }

    def rg(
        self,
        pattern: str,
        *,
        path: Optional[str] = None,
        glob: Optional[str] = None,
        ignore_case: bool = False,
        smart_case: bool = False,
        word: bool = False,
        invert: bool = False,
        context: Optional[int] = None,
        before: Optional[int] = None,
        after: Optional[int] = None,
        max_matches: Optional[int] = None,
        max_files: Optional[int] = None,
        files_only: bool = False,
        counts: bool = False,
        long: bool = False,
        fresh: bool = False,
        ignore: Optional[Iterable[str]] = None,
    ) -> Dict[str, Any]:
        """Bounded ripgrep-style search. ``files_only`` is ``-l``, ``counts`` is ``-c``."""
        params = self._rg_params(pattern, path, glob, ignore_case, smart_case, word, invert, context, before, after, max_matches, max_files, long)
        params["l"] = files_only
        params["c"] = counts
        return self._verb("rg", params, fresh=fresh, ignore=ignore)

    def rg_files(self, pattern: str, **kwargs: Any) -> Dict[str, Any]:
        """``rg -l``: only the files that match — the cheapest way to locate code."""
        return self.rg(pattern, files_only=True, **kwargs)

    def rg_counts(self, pattern: str, **kwargs: Any) -> Dict[str, Any]:
        """``rg -c``: match counts per file."""
        return self.rg(pattern, counts=True, **kwargs)

    # -- chained helpers -----------------------------------------------------------

    def read_symbol(self, path: str, name: str, *, kind: Optional[str] = None, padding: int = 0) -> Optional[Dict[str, Any]]:
        """Read one symbol's source by name: ``outline`` then ``cat(lines=...)``. Two requests."""
        outline = self.outline(path)
        symbol = next(
            (s for s in outline["symbols"] if s["name"] == name and (kind is None or s["kind"] == kind)),
            None,
        )
        if symbol is None:
            return None
        pad = max(0, padding)
        start = max(1, symbol["line"] - pad)
        end = min(outline["total_lines"], symbol["end_line"] + pad)
        code = self.cat(path, lines=(start, end))
        code["symbol"] = symbol
        return code

    def context(
        self,
        pattern: str,
        *,
        window: int = 20,
        max_snippets: int = 5,
        **rg_kwargs: Any,
    ) -> List[Dict[str, Any]]:
        """Locate then read: ``rg`` for the pattern, then one ``cat`` window per matching file."""
        rg_kwargs.setdefault("max_matches", max(1, max_snippets) * 4)
        hits = self.rg(pattern, **rg_kwargs)
        seen: set = set()
        snippets: List[Dict[str, Any]] = []
        for match in hits.get("matches", []):
            if match["is_context"] or match["path"] in seen:
                continue
            seen.add(match["path"])
            code = self.cat(match["path"], lines=(max(1, match["line"] - window), match["line"] + window))
            snippets.append(
                {
                    "path": match["path"],
                    "start_line": code["start_line"],
                    "end_line": code["end_line"],
                    "text": code["text"],
                    "commit": code["commit"],
                }
            )
            if len(snippets) >= max(1, max_snippets):
                break
        return snippets
