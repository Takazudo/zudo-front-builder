# Helper adapters

Each variant is one file copied to `app/lib/content.ts` when the fixture is
materialized. It must export `getIndex(): { articles, bySlug }`. Route code is
identical for every variant.

Files are stored as `content.ts.adapter`, which the repo formatter does not
own. A `.ts` name would let `vp fmt` (printWidth 100) and the lefthook
pre-commit hook rewrite the byte-exact original.

| Variant | Provenance | Description |
| --- | --- | --- |
| `A0` | reconstructed | Direct read: rebuilds the index on every call |
| `A1` | original-input | Exact bytes of the original memoized helper (sha256 `10cb58db…38a6`) |
| `A2` | derived from A1 | A1 plus `void getContentSnapshot()?.collections.articles;` before the cache check |
| `A3` | reconstructed diagnostic | A1 that rebuilds when the snapshot's `articles` slot identity changes |

A variant whose manifest entry carries `sha256` is rejected by
`verify-identity` unless the file matches it. Register a new variant in
`../manifest.json` under `adapters.variants`.
