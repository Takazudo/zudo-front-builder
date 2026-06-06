# zfb-md-extras

Remark/rehype plugin ports for the `zfb` markdown pipeline: github-alerts,
github-autolinks, code enrichment, code tabs, heading-marker TOC, TOC export,
transclusion, ruby annotations, mermaid, reading time, image dimensions, and
link validation.

---

## Fixture-based test workflow

Each feature port uses a snapshot workflow to stay faithful to the upstream
JS reference output.

### Fixture directory layout

```text
crates/zfb-md-extras/tests/fixtures/<feature-name>/
├── input.md          — Markdown source
├── expected.html     — Expected HTML output (committed; generated once)
└── normalize.txt     — Optional verbatim opt-out (see below)
```

### Generating `expected.html`

Run the generation script once, passing the upstream npm plugin name and
the fixture's input file:

```sh
# Direct-function mode (plugin exports a (markdown: string) => string fn)
node crates/zfb-md-extras/scripts/gen-fixture.mjs \
  --plugin remark-gfm \
  --input  crates/zfb-md-extras/tests/fixtures/gfm/input.md \
  --output crates/zfb-md-extras/tests/fixtures/gfm/expected.html

# Remark/unified pipeline mode
node crates/zfb-md-extras/scripts/gen-fixture.mjs \
  --plugin remark-gfm \
  --remark \
  --input  ... \
  --output ...
```

Commit both `input.md` and `expected.html`. The Rust integration test reads
the committed file — `gen-fixture.mjs` is never called by CI.

### Running the integration test

```sh
cargo test --package zfb-md-extras
```

### Writing a new fixture test

```rust
// tests/my_feature.rs
use zfb_md_extras::test_harness::run_fixture;

#[test]
fn test_my_feature() {
    let fixtures = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/my-feature"
    );
    run_fixture(fixtures, |input| {
        markdown::to_html(input)
    });
}
```

### HTML normalization

`run_fixture` passes both the actual output and `expected.html` through
`zfb_test_utils::normalize_html` before comparing. This tolerates minor
serialization differences between the upstream JS plugin and the Rust port:

| Category | Canonical form |
|---|---|
| Attribute order | Sorted lexicographically by attribute name |
| Entity encoding | `&amp;` `&lt;` `&gt;` `&quot;` `&nbsp;` only; `&apos;`/`&#x27;` → literal `'` |
| Boolean attributes | Empty-string form: `disabled=""` |
| Self-closing / void | HTML5 form: `<br>` (no `/>`) |
| Inter-element whitespace | Pure-whitespace text nodes → single newline |
| Literal text contexts | `<pre>` `<code>` `<textarea>` `<script>` `<style>` — never touched |

### Verbatim opt-out

Add a `normalize.txt` file containing the single word `verbatim` to disable
normalization for a specific fixture. Both sides are then compared as raw
strings:

```text
# tests/fixtures/edge-case/normalize.txt
verbatim
```

This is useful when the fixture tests exact whitespace or entity encoding
that normalization would obscure.
