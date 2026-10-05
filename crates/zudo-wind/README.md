# zudo-wind

`zudo-wind` is the owned, versioned utility CSS language engine for zfb. It
defines candidate syntax, variant vocabulary, structured diagnostics, rule
metadata, extraction, compilation, explanation, and audit APIs.

The public `structural_split` function separates one class token without a
configured vocabulary. `parse_candidate` additionally checks configured
breakpoints, dark mode, supported variants, and canonical variant order.
`explain` returns deterministic structured and plain or JSON renderings for a
candidate. `audit` consumes extracted occurrences and reports ordinary class
names, dead utilities, declaration conflicts within a class literal, and
dynamic constructions without treating low-confidence literals as class
attributes.

The language contract is the workspace
[zudo-wind v1 specification](../../research/3242-zudo-wind-v1-spec.md).

## Catalog export

The committed
[`catalog/zudo-wind-catalog.v1.json`](catalog/zudo-wind-catalog.v1.json) file
exports all v1 catalog entries and their examples. Its independent catalog
schema version is `1`; the language spec version is `1`. Export fields use
stable camelCase names and the entries follow the catalog's deterministic
conflict and order ranks. The stale test compares parsed JSON values so Oxfmt
can own the checked-in formatting.

Regenerate the file from the workspace root, then format it:

```text
cargo run -p zudo-wind --example export_catalog
pnpm exec vp fmt --write crates/zudo-wind/catalog/zudo-wind-catalog.v1.json
```
