# Native stationary regions (#4205)

Capability verdict: **existing APIs sufficient** for the bounded ZFB fixture.
No runtime change or new public API is required. Consumer CSS owns motion policy;
native names identify visual regions; existing persistence APIs retain DOM/islands.

The fixture generator uses built workspace exports. The separate
`check-native-regions-packed.mjs` packs the exact local SDK, runtime and slugify,
installs those tarballs in an isolated temporary directory, checks public export
resolution, and runs the same browser spec against those installed files. It does
not patch dependencies or copy the router. Neither fixture is a CLI build.

After the builds and fixture generation in `pnpm test:router-chromium`, run:

```sh
pnpm exec playwright test --config tests/router-chromium/playwright.config.mjs native-regions.chromium.spec.mjs
pnpm exec node tests/router-chromium/check-native-regions-packed.mjs
```

The ordinary router Chromium workflow runs both the full suite and packed check.
It preserves bounded PNG/JSON native evidence for seven days. The packed manifest
records source revision, tarball hashes and resolved entrypoints. Browser records
are included with the native evidence. A source revision alone does not represent
uncommitted test edits during local development.

## Coverage and oracle

- Static replacement, island in persisted ancestor, public `Island persist`:
  fresh destination props, expected remount/disposal and working interactions.
- Matching/changing context, one-sided entry/exit, Back/Forward, superseding and
  explicitly skipped transitions; reduced motion and no-native fallback.
- Existing router specs retain history, forms, accessibility and island lifetime
  coverage rather than multiplying all permutations here.
- `before-swap` registers promise observers and returns. It never awaits `ready`
  inside the update callback. Ready, DOM-update and finished outcomes are separate.
- CDP inspects actual native group/old/new pseudo-nodes and their parent. Active
  frames use animations-allowed screenshots with distinguishable changed content.
  Paused/seeked native animations provide reproducible intermediate frames; a
  separate unpaused test proves browser-clock progression and completion.
- An explicit animated-content control is mandatory. Missing pseudo identifiers
  or empty animation filters cannot pass. Each old/new/group protection removal
  independently fails the same stationary oracle, with deliberate test-only motion
  that makes even an outgoing image under an opaque incoming image observable.
- A named-region removal demonstrates capture in the animated root ancestor.

The recipe assumes stable layout and opaque region backgrounds. Transparent
incoming pixels may expose outgoing content and need separately tested compositing.
It does not promise fixed coordinates across layout changes or arbitrary authored
ancestor animations. Fallback/reduced-motion correctness is not native proof.

Actual CLI evidence is a separate strict EN/JA docs build and emitted HTML checks,
using the repository's pinned published CLI. The upstream fixture does not verify
zudo-doc's real application or establish a historical 4.3.3 to 4.4.0 regression.
