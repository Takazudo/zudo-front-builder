# routing-rendering fixture

Representative page, layout, component, and content files used by
`crates/zfb-render/tests/integration_routing_rendering.rs`. The current test
scans `pages/` and asserts that the expected static, dynamic, catchall, and
paginated route templates are discovered.

## Layout

```text
pages/
├── index.tsx
├── about.tsx
├── blog/
│   ├── index.tsx
│   ├── [slug].tsx
│   └── page/[page].tsx
├── docs/[...slug].tsx
├── manual/[[...slug]].tsx
└── [lang]/[slug].tsx
layouts/
├── default.tsx
└── blog.tsx
components/header.tsx
content/posts.ts
```

The fixture components use JSX and plain props. The active test validates route
scanning; a full render harness is owned by the build-pipeline tests.
