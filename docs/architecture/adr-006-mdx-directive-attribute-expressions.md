# ADR-006: MDX directive attribute expressions stay string-literal-only in v1

- **Status:** Accepted
- **Date:** 2026-04-28
- **Owners:** Epic [`zfb-cli-dx`] sub-task 5 (mdx-directive-attrs); follow-up to [PR #45](https://github.com/Takazudo/zudo-front-builder/pull/45) (DirectiveRegistry).
- **Related:** Issue #54 (epic), super-epic [zudolab/zudo-doc#473](https://github.com/zudolab/zudo-doc/issues/473) — Astro→zfb migration, Phase B E6.2 (zudo-doc port).

## Decision (one sentence)

**The DirectiveRegistry contract stays at v1 — directive `[label]` and `{attrs}` values are plain string literals only. No frontmatter interpolation, no expression substitution, no JSX expression attributes — because the existing zudo-doc content does not use any of those patterns and would not benefit from the feature.**

## Context

`crates/zfb-content/src/plugins/directives.rs` (the `DirectiveRegistry` plugin shipped in #45) recognises three CommonMark-Directives shapes:

- container — `:::name[label]{attrs}` … `:::`
- leaf — `::name[label]{attrs}`
- text — `:name[label]{attrs}`

The v1 contract emits every attribute as a JSX **string-literal** attribute (`title="foo"`). Raw-expression attributes such as `title={someVar}` are explicitly NOT supported (`AttributeValue::Literal` only — see the `## Attribute escaping (v1)` block in `directives.rs` lines 24-32).

Plain-string label works today:

    :::note[Plain title]
    body
    :::

Frontmatter / expression interpolation does not:

    :::note[Title with {var}]   ← would emit a literal "{var}" string, not interpolate
    body
    :::

The question for this sub-task: do we extend v1 to support frontmatter-field interpolation in directive attributes (path A — implement registry extension), or do we ratify v1's restriction (path B — ADR + close)?

The decision criterion the epic plan (#54) sets: **the zudo-doc port (Phase B E6.2) must not require workarounds for content that exists today**. So the question reduces to: does today's zudo-doc content actually use `{…}` patterns inside directive labels or attributes?

## Grep evidence

zudo-doc was shallow-cloned (`gh repo clone zudolab/zudo-doc /tmp/zudo-doc-grep -- --depth=1`) on 2026-04-28 and grepped for every plausible variant of "directive label or attr block contains a `{…}` expression":

    # Container with {…} inside the [label]
    grep -rE ':::[a-zA-Z]+\[[^]]*\{[^}]*\}[^]]*\]' \
      /tmp/zudo-doc-grep --include='*.md' --include='*.mdx'
    → 0 hits

    # Leaf with {…} inside the [label]
    grep -rE '^::[a-zA-Z]+\[[^]]*\{[^}]*\}[^]]*\]' \
      /tmp/zudo-doc-grep --include='*.md' --include='*.mdx'
    → 0 hits

    # Inline text directive with {…} inside the [label]
    grep -rE '[^:]:[a-zA-Z][a-zA-Z0-9_-]*\[[^]]*\]\{[^}]*\}' \
      /tmp/zudo-doc-grep --include='*.md' --include='*.mdx'
    → 0 hits

    # ANY use of the braced {attrs} block on a directive
    grep -rE ':::[a-zA-Z]+(\[[^]]*\])?\{[^}]*\}' \
      /tmp/zudo-doc-grep --include='*.md' --include='*.mdx'
    → 0 hits

For scale, the same repo contains:

    # Total :::name directive opens (any flavour)
    grep -rE '^:::[a-zA-Z]+' /tmp/zudo-doc-grep \
      --include='*.md' --include='*.mdx' | wc -l
    → 50

    # Directive opens with a [label]
    grep -rE '^:::[a-zA-Z]+\[' /tmp/zudo-doc-grep \
      --include='*.md' --include='*.mdx' | wc -l
    → 22

So: 50 directive uses, 22 with explicit labels — every one of those 22 labels is a plain string literal. Zero use of expression interpolation. Zero use of the `{attrs}` block.

The 22 labelled uses live across these files:

- `src/content/docs/components/admonitions.mdx` (10) — English admonition fixtures (`[Custom Title]`, `[Pro Tip]`, `[Did You Know?]`, `[Deprecation Notice]`, `[Breaking Change]`).
- `src/content/docs-ja/components/admonitions.mdx` (10) — Japanese mirrors (`[カスタムタイトル]`, `[便利なヒント]`, `[豆知識]`, `[非推奨のお知らせ]`, `[破壊的変更]`).
- `src/content/docs/getting-started/writing-docs.mdx` (1) — `[Optional Title]`.
- `src/content/docs-ja/getting-started/writing-docs.mdx` (1) — `[任意のタイトル]`.

Auxiliary occurrences in `e2e/fixtures/smoke/.../admonitions-test.mdx` and `.claude/skills/zudo-doc-writing-rules/SKILL.md` were also surveyed and follow the same plain-string pattern.

The cloned repo at `/tmp/zudo-doc-grep` was removed after the grep run.

## Decision details

1. The DirectiveRegistry v1 contract documented at `crates/zfb-content/src/plugins/directives.rs` lines 24-32 stands as-is. No code change in this sub-task.
2. The zudo-doc port (Phase B E6.2) does **not** need a directive-attribute interpolation feature. All existing labels port verbatim.
3. If a future zudo-doc page (or any other consumer) introduces a labelled directive that needs to interpolate a frontmatter field, the path forward is **frontmatter-field interpolation only** — i.e. add a registry extension that resolves `{fieldName}` placeholders against the current page's frontmatter, with no support for arbitrary expressions (no `{1 + 2}`, no method calls, no JSX expressions). That extension would be a v1.5 contract bump and would need its own ADR; this ADR pre-commits to *that* shape rather than to general expression evaluation, because:
   - Frontmatter interpolation is auditable and side-effect-free; arbitrary expressions are not.
   - Frontmatter interpolation matches the failure mode of static-site rendering (string-substitute at build time), where arbitrary expressions invite "works on my machine" behaviour drift between SSG and SSR adapters.
   - The MDX expression-attribute escape hatch (`<Note title={someVar}>…</Note>`) is already available for any author who needs full expression power; the directive shorthand does not have to grow into a second expression-evaluation surface.
4. If — and only if — a real consumer surfaces a need that the JSX escape hatch cannot meet, revisit this ADR and implement the `{fieldName}`-only registry extension described above.

## Consequences

**Positive.**

- Zero new code, zero new dep, zero new attack surface in v1.
- The DirectiveRegistry contract's escaping guarantee (every value emits as a JSX string literal — `"`, `&`, `<`, `>`, line terminators escaped) holds unconditionally. There is no path through which user input becomes an executable expression.
- Phase B E6.2 (zudo-doc port) proceeds with no blocker.

**Negative — costs we accept.**

- An author who *does* want a directive label parameterised by a frontmatter field has to reach for the JSX escape hatch (`<Note title={pageTitle}>…</Note>`) instead of the `:::note[{pageTitle}]` shorthand. This is a known cost; the escape hatch covers the case completely.
- A future content batch may genuinely want the `{fieldName}` shorthand. When that happens, this ADR is the bookmark for "what was decided and why", and the v1.5 extension shape is pre-specified.

**Neutral.**

- The existing v1 docstring in `directives.rs` (the `## Attribute escaping (v1)` block) already states the restriction. This ADR cites that block rather than re-restating it, so there is no doc-divergence risk.

## Alternatives considered

### PATH A — Implement frontmatter-field interpolation now

Rejected for v1. The grep evidence shows zero existing usage. Implementing a registry extension, fixtures, unit tests, and contract-doc bump *for content that does not exist* would be speculative and would commit zfb to a contract surface (placeholder syntax, missing-field semantics, escape-rule interaction with the existing JSX literal escaping) that no real consumer has shaped. Defer until a concrete consumer pins the requirements.

### Implement arbitrary expression evaluation (`{1 + 2}`, method calls)

Rejected outright, even as a future direction. It duplicates the JSX expression-attribute escape hatch (which is already available via `<Component prop={…}>…</Component>` syntax and is evaluated by the downstream MDX runtime, not by the directive registry). Carrying a second expression evaluator inside the directive registry would be a second surface to keep in sync with MDX's evaluation semantics — strictly worse than letting authors who need expressions drop into JSX.

### Document the restriction in `directives.rs` rustdoc only, no ADR

Rejected. The rustdoc is already accurate (and is cited above). The reason this needs an ADR rather than living only in code comments is the *decision against PATH A* — i.e. the explicit choice not to add frontmatter interpolation, and the pre-commitment that any future extension stays scoped to frontmatter fields. That decision belongs in the architecture record, not in a function-level docstring.

## References

- `crates/zfb-content/src/plugins/directives.rs` lines 24-32 — the v1 string-literal-only docstring this ADR ratifies.
- PR #45 — original DirectiveRegistry implementation.
- Issue #54 — epic tracking sub-task 5 (this ADR).
- Super-epic [zudolab/zudo-doc#473](https://github.com/zudolab/zudo-doc/issues/473) — Astro→zfb migration, Phase B E6.2 (zudo-doc port consumer).
