// Sample TypeScript file exercising the `zfb/content` surface declared
// by `emit_types_dts` (Sub 48). This file is referenced from the
// `dts_typechecks_against_sample_fixture` test in
// `crates/zfb-content/src/collection.rs`. It is NOT compiled by
// `cargo test` — the test asserts symbol presence via string match.
// To verify it really typechecks, point `tsc` at it together with the
// emitted `types.d.ts` (single-collection input `["blog"]`).
//
// Example (manual):
//   cargo run --example emit-dts > /tmp/types.d.ts
//   tsc --noEmit --strict --types . /tmp/types.d.ts sample.ts

import { getCollection, getEntry } from "zfb/content";

// `K extends keyof ZfbCollections` infers to the literal "blog".
const allBlog = getCollection("blog");
// Element shape: { slug: string; data: Record<string, unknown>; body: string }
const slugs: string[] = allBlog.map((entry) => entry.slug);
const bodies: string[] = allBlog.map((entry) => entry.body);
const titles: unknown[] = allBlog.map((entry) => entry.data["title"]);

// `getEntry` returns `T | undefined` so callers must narrow.
const maybeOne = getEntry("blog", "hello-world");
const oneSlug: string | undefined = maybeOne?.slug;
const oneBody: string | undefined = maybeOne?.body;

// Re-export so the symbols are observable to a downstream typechecker
// even when run with `--noUnusedLocals`.
export { allBlog, slugs, bodies, titles, maybeOne, oneSlug, oneBody };
