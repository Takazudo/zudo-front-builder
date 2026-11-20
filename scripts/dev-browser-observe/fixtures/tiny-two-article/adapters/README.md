# Helper adapters

Each variant is one file copied to `app/lib/content.ts` when the fixture is
materialized. It must export `getIndex(): { articles, bySlug }`.

| Variant | File | Status |
| --- | --- | --- |
| `A0` | `A0/content.ts` | Reconstructed direct-read helper |

Add a variant as `<ID>/content.ts` and register it in `../manifest.json`
under `adapters.variants`. The run profile then selects it by ID.
