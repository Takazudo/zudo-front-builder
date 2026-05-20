# routing-rendering fixture project

End-to-end fixture for the Epic 3 routing + rendering pipeline.
Consumed by `tests/integration_routing_rendering.rs`.

## Layout

```
pages/
├── index.tsx                # static
├── about.tsx                # static (acceptance)
├── blog/
│   ├── index.tsx            # static (lists posts)
│   ├── [slug].tsx           # dynamic, paths() over content/posts.ts
│   └── page/
│       └── [page].tsx       # pagination via paginate() from "zfb"
├── docs/
│   └── [...slug].tsx        # catchall
└── [lang]/
    └── [slug].tsx           # nested dynamic (cartesian product)
layouts/
├── default.tsx              # used by most pages
└── blog.tsx                 # used when `meta.layout === "blog"`
components/
└── header.tsx               # portable component (no framework-specific APIs)
content/
└── posts.ts                 # stub blog post data
snapshots/
├── preact/                  # populated by INSANE_UPDATE_SNAPSHOTS=1
└── react/                   # populated by INSANE_UPDATE_SNAPSHOTS=1
```

## Portable-component contract

Every component / page / layout in this fixture is **portable** — it
relies only on JSX and plain props, never on Preact-only or React-only
APIs (no `signals`, no `useResource`, no Preact-specific hooks, no
React-specific server hooks). That's why the harness can assert
**byte-identical HTML** between `framework: "preact"` and
`framework: "react"`.

If a future test needs to exercise framework-specific behaviour, add
it as a separate fixture under `tests/fixtures/` with a
non-cross-framework assertion shape.

Concrete portability rules in this fixture:

- Use `className` (not `class`); both Preact and React accept it,
  and both render it as `class="…"` in HTML.
- No signals, no `useResource`, no `@preact/signals-react`.
- No React-only server hooks (`useFormStatus`, etc.).

## Snapshot bootstrap

Snapshots are generated on first run via the existing
`INSANE_UPDATE_SNAPSHOTS=1` convention (same one used by
`zfb-content`'s pipeline tests). After Subs 1–7 land and the manager
merges them into `base/zfb-init-routing-rendering`, run:

```sh
INSANE_UPDATE_SNAPSHOTS=1 cargo test -p zfb-render \
    --test integration_routing_rendering
```

Inspect the diffs, commit the populated snapshot files, then re-run
without the env var to confirm the comparison passes.

## Why this won't build on the sub08-tests worktree alone

`zfb-render` and `zfb-router` are produced by Subs 2 and 3 of Epic 3.
Until those land on the epic base branch, this crate has unresolved
imports and `cargo build -p zfb-render` will fail. That's expected.
The harness compiles only after the manager's Step 9 merge.
