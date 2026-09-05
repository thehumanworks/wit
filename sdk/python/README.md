# wit-api (Python)

Zero-dependency client for the [wit URL API](../../showcase/url-api/README.md):
read any GitHub repository without cloning, with token budgets you control.
Standard library only (`urllib`), Python 3.9+.

```python
from wit_api import WitClient

wit = WitClient(token=os.environ.get("GITHUB_TOKEN"))
repo = wit.repo("ratatui/ratatui")            # or wit.repo("o/r", "v0.29.0")

stats = repo.stats()                                          # size / tokens / languages, no blob reads
files = repo.rg_files("impl Widget for", glob="*.rs")
outline = repo.outline(files["files"][0]["path"])             # symbols with line ranges
impl = next(s for s in outline["symbols"] if s["kind"] == "impl")
code = repo.cat(files["files"][0]["path"], lines=(impl["line"], impl["end_line"]))
print(code["commit"], code["text"])                           # provenance travels with every result
```

Higher-level helpers that chain calls for you:

- `repo.read_symbol(path, name, kind=..., padding=...)` → outline + cat range (2 requests).
- `repo.context(pattern, window=..., max_snippets=..., glob=...)` → rg + one cat window per file.
- `wit.search("terminal ui", lang="rust")` → find `owner/repo` first.
- `repo.text("tree", path="src", l=1)` → the CLI-identical plaintext for any verb.

Errors raise `WitError` with `status`, `code`, `retry_after` and
`is_rate_limited`; pass `retries=2` to have 429s retried while honouring
`retry-after`. Inject `transport=` to test without a network.

## Develop

```bash
cd sdk/python
python -m unittest discover -s tests -v
```
