# Field Notes fixture (`field-notes-fixture.v1`)

This fixture describes the original 100-article Field Notes ZFB app. The app
and its corpus are **not** committed here. A profile names the source
directory in `fixture.source.dir` and pins its tree hashes. `cli.mjs pins`
prints those hashes, and `verify-identity` checks them.

This directory holds only the parts the harness owns:

| Path | Purpose |
| --- | --- |
| `manifest.json` | Source layout, adapter provenance, selectors and the CSS markers |
| `adapters/original-memoized/content.ts.adapter` | Exact original helper (`c104d8f0…`, `original-input`) |
| `adapters/recovered-snapshot/content.ts.adapter` | Exact recovered archive helper (`7ff8a753…`) |
| `adapters/direct-read/content.ts.adapter` | Reconstructed control, rebuilt on every call |
| `managed/global.css.src` | Optional managed stylesheet (`fixture.managedCss: true`) |
| `overlays/internal-placement/zfb.config.ts.overlay` | Derived config for `fixture.placement: "internal"` (`ddce03ff…`, base `a68c2bdb…`; diff in `manifest.json` `placements.internal`) |

Adapters, the managed stylesheet and the overlay use non-`.ts`/`.css` names so the repo
formatter never changes their bytes.
