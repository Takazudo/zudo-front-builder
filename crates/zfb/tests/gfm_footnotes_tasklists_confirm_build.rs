//! Wave 6 confirm pass for the GFM Footnotes And Task Lists epic (zfb#2021,
//! this test for issue #2029).
//!
//! Waves 1-5 taught zfb to emit GFM task-list checkboxes and footnotes
//! instead of silently dropping them, across THREE emit sites:
//!
//! 1. `crates/zfb-content/src/pipeline.rs`'s `mdast_to_hast_inner` (the
//!    hast-bridge path, driven by `Pipeline::run`/`run_with_context`).
//! 2. `crates/zfb-content/src/mdx_jsx_emit.rs`'s `emit_node` (the top-level
//!    document-body JSX-emit path).
//! 3. `crates/zfb-content/src/mdx_jsx_emit.rs`'s `jsx_render_child` /
//!    `jsx_raw_recursive` (markdown nested inside an MDX JSX element's
//!    children, e.g. `<Note>…</Note>`).
//!
//! ## Which paths a real `zfb build` can actually exercise
//!
//! Grepping every call site of `Pipeline::run` / `Pipeline::run_with_context`
//! outside `pipeline.rs`'s own test module turns up exactly two production
//! callers: `zfb-md-wasm`'s `renderHtml()` API, and nothing else — NOT
//! `zfb-build`'s bundler, NOT `zfb-render`'s embedder loader (whose own doc
//! comment states it is "a separate library/embedder path not used by the
//! zfb CLI at all"). `zfb-content::collection`'s own doc comment confirms
//! the CLI-facing contract directly: "`.md` and `.mdx` files take the same
//! MDX→JSX path (CommonMark is a strict MDX subset)" — for BOTH content
//! collection entries (`walk_collection_with_cache`) AND `.md`/`.mdx` PAGE
//! files (`materialise_shadow`'s `_zfb_md_body_<stem>.jsx` compiled-body
//! shape, proven by `bundler.rs`'s own
//! `materialise_shadow_compiles_md_pages_and_emits_shell` unit test).
//!
//! So site 1 (the hast-bridge) is **not reachable through `zfb build`/`zfb
//! dev` at all** — it is exclusively a `zfb-md-wasm` surface, and is already
//! confirmed there by `crates/zfb-md-wasm/npm/test/gfm-parity.test.ts`'s
//! `renderHtml`-tier fixtures (`task-list`, `footnotes`), which check 2 of
//! this confirm pass (`pnpm -r test`) re-runs on every pass. This test
//! covers what a real `zfb build` CAN reach: sites 2 and 3, through both the
//! `.md` and `.mdx` file extensions.
//!
//! ## What this test proves
//!
//! A fresh `zfb build` (no `pnpm install` needed — same embedded-fallback
//! contract as `end_to_end_basic_blog_build.rs`) against a small content
//! collection:
//!
//! - `tasklist-md.md` / `tasklist-mdx.mdx` — byte-identical bodies (checked,
//!   unchecked, nested, and a plain non-task sibling item) compiled through
//!   site 2. Asserts the two extensions' rendered `<main>` bodies are
//!   **byte-identical** (after normalising the one legitimately-different
//!   attribute, `data-slug`) — proving task-list emission does not
//!   distinguish `.md` from `.mdx` on the real build path.
//! - `footnotes-md.md` / `footnotes-mdx.mdx` — byte-identical bodies with a
//!   **repeated reference** (`[^a]` cited twice, `[^b]` once). Same
//!   byte-identical assertion, PLUS structural assertions: numbering by
//!   first reference (`a` → 1, `b` → 2), definitions rendered in reference
//!   order, and the repeated reference producing two distinct occurrence
//!   ids (`user-content-fnref-a`, `user-content-fnref-a-1`) that both link
//!   to the SAME definition (`user-content-fn-a`) and both carry backrefs
//!   pointing back at their own occurrence.
//! - `nested.mdx` — task-list items AND a footnote reference/definition
//!   nested inside a custom `<Note>` JSX component's children, exercising
//!   site 3. Asserts SEMANTIC equivalence with the top-level fixtures
//!   (same checked/unchecked states; same numbering/id/backref structure)
//!   rather than a byte match, and asserts the footnote **section still
//!   renders once, at the end of the whole document, outside `</aside>`**
//!   — proving the document-level footnote model isn't confused by a
//!   reference living inside a JSX subtree.
//!
//! ## Where the two JSX-emit sites legitimately differ
//!
//! Empirically diffing the nested (`jsx_render_child`) output against the
//! top-level (`emit_node`) output for otherwise-identical content surfaces
//! two real, harmless serialization differences that this test tolerates
//! rather than papering over:
//!
//! - The nested path's footnote marker renders `data-footnote-ref="true"`;
//!   the top-level path renders the bare `data-footnote-ref` (no value).
//! - The nested path inserts a literal space between a task-list checkbox
//!   and its following `<p>` (`<input .../> <p>`); the top-level path does
//!   not (`<input.../><p>`).
//!
//! Neither difference changes checked state, numbering, or link structure,
//! so this test asserts on those semantics rather than forcing byte
//! equality across the two sites.
//!
//! ## Tiering
//!
//! Level 4 (real `zfb build` process e2e). No external binary is required —
//! the fixture has no Tailwind config and no "use client" islands, so the
//! build never invokes esbuild or tailwindcss-v4 (confirmed empirically: a
//! local run with neither `ZFB_ESBUILD_BIN` nor `ZFB_TAILWIND_BIN` set
//! succeeds and logs "no islands found; skipping islands bundle"). Not
//! `#[ignore]`d, matching `end_to_end_basic_blog_build.rs`'s own
//! no-external-binary tier. Added to `.config/nextest.toml`'s `e2e-heavy`
//! test-group as a build-only member (spawns a real `zfb build` process;
//! serialized alongside the other build-command binaries).

use std::fs;
use std::path::Path;
use std::process::Command;

use zfb_test_utils::zfb_binary;

/// `true` when the non-zero build is a known-skip (no embedded V8 / no
/// esbuild / no tailwindcss-v4 binary), matching the skip pattern used
/// across the sibling build-command tests.
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8")
        || combined.contains("no esbuild")
        || combined.contains("no tailwind")
        || (combined.contains("tailwindcss") && combined.contains("not found"))
}

fn write_fixture(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "markdown": { "gfm": true },
  "collections": [{ "name": "notes", "path": "content/notes" }]
}
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("content").join("notes")).unwrap();
    fs::create_dir_all(root.join("pages").join("notes")).unwrap();
    fs::create_dir_all(root.join("components")).unwrap();

    let task_list_body = "- [ ] Buy milk\n\
        - [x] Walk the dog\n\
        - [ ] Nested parent\n\
        \x20\x20- [x] Nested child\n\
        - Plain non-task item\n";
    fs::write(root.join("content/notes/tasklist-md.md"), task_list_body).unwrap();
    fs::write(root.join("content/notes/tasklist-mdx.mdx"), task_list_body).unwrap();

    // A repeated reference (`[^a]` cited twice) is the acceptance-critical
    // shape: it must share one number but mint two distinct occurrence ids.
    let footnotes_body = "First footnote[^a] and a second[^b], \
        then a repeat of the first[^a] again.\n\n\
        [^a]: Definition A.\n\n\
        [^b]: Definition B.\n";
    fs::write(root.join("content/notes/footnotes-md.md"), footnotes_body).unwrap();
    fs::write(root.join("content/notes/footnotes-mdx.mdx"), footnotes_body).unwrap();

    // `.md` cannot host JSX components at all — this shape is `.mdx`-only
    // by construction, exercising site 3 (`jsx_render_child` /
    // `jsx_raw_recursive`) rather than site 2.
    fs::write(
        root.join("content/notes/nested.mdx"),
        "<Note title=\"Nested\">\n\n\
        - [ ] Nested task unchecked\n\
        - [x] Nested task checked\n\n\
        Ref inside note[^n].\n\n\
        [^n]: Nested footnote body.\n\n\
        </Note>\n",
    )
    .unwrap();

    fs::write(
        root.join("components/note.tsx"),
        r#"import type { ComponentChildren } from "preact";

type Props = {
  title?: string;
  children: ComponentChildren;
};

export default function Note({ title, children }: Props) {
  return (
    <aside class="admonition" data-component="note">
      {title ? <strong>{title}</strong> : null}
      <div class="admonition__body">{children}</div>
    </aside>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("pages/notes/[slug].tsx"),
        r#"import { defaultComponents } from "@takazudo/zfb";
import Note from "../../components/note";

export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const notes = await getCollection("notes");
  return notes.map((n) => ({ params: { slug: n.slug }, props: { note: n } }));
}

export default function NotePage({ note }) {
  return (
    <main data-slug={note.slug}>
      <note.Content components={{ ...defaultComponents, Note }} />
    </main>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("pages/index.tsx"),
        "export default function Index() { return <main>index</main>; }\n",
    )
    .unwrap();
}

fn read_page(dist: &Path, rel: &str) -> String {
    let path = dist.join(rel).join("index.html");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Strips the one legitimately-different attribute (`data-slug="…"`) so two
/// pages compiled from byte-identical bodies under different filenames can
/// be compared for byte-identical output.
fn strip_slug_attr(html: &str) -> String {
    let needle_start = html
        .find("data-slug=\"")
        .expect("expected data-slug attribute");
    let after_open_quote = needle_start + "data-slug=\"".len();
    let end_quote = html[after_open_quote..]
        .find('"')
        .map(|i| after_open_quote + i)
        .expect("expected closing quote for data-slug");
    format!("{}{}", &html[..needle_start], &html[end_quote + 1..])
}

#[test]
fn gfm_footnotes_and_task_lists_confirm_build() {
    let tmp = tempfile::tempdir().expect("create tempdir for fixture");
    let root = tmp.path();
    write_fixture(root);

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .output()
        .expect("spawn zfb binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    if !output.status.success() {
        if is_known_skip(&combined) {
            eprintln!(
                "[gfm_footnotes_and_task_lists_confirm_build] zfb build exited \
                 non-zero with a known-skip indicator; skipping test.\n\
                 stdout: {stdout}\nstderr: {stderr}"
            );
            return;
        }
        panic!(
            "zfb build failed unexpectedly.\nstatus: {:?}\nstdout: {stdout}\nstderr: {stderr}",
            output.status,
        );
    }

    let dist = root.join("dist");

    // --- Task lists: `.md` vs `.mdx`, byte-identical modulo data-slug ---
    let tasklist_md = read_page(&dist, "notes/tasklist-md");
    let tasklist_mdx = read_page(&dist, "notes/tasklist-mdx");
    assert_eq!(
        strip_slug_attr(&tasklist_md),
        strip_slug_attr(&tasklist_mdx),
        "task-list rendering must be byte-identical between .md and .mdx \
         (modulo data-slug):\n--- md ---\n{tasklist_md}\n--- mdx ---\n{tasklist_mdx}"
    );
    assert_eq!(
        tasklist_md,
        "<main data-slug=\"tasklist-md\">\
         <ul>\
         <li><input type=\"checkbox\" disabled/><p>Buy milk</p></li>\
         <li><input type=\"checkbox\" disabled checked/><p>Walk the dog</p></li>\
         <li><input type=\"checkbox\" disabled/><p>Nested parent</p>\
         <ul><li><input type=\"checkbox\" disabled checked/><p>Nested child</p></li></ul>\
         </li>\
         <li><p>Plain non-task item</p></li>\
         </ul>\
         </main>",
        "unexpected task-list HTML shape: {tasklist_md}"
    );

    // --- Footnotes: `.md` vs `.mdx`, byte-identical modulo data-slug ---
    let footnotes_md = read_page(&dist, "notes/footnotes-md");
    let footnotes_mdx = read_page(&dist, "notes/footnotes-mdx");
    assert_eq!(
        strip_slug_attr(&footnotes_md),
        strip_slug_attr(&footnotes_mdx),
        "footnote rendering must be byte-identical between .md and .mdx \
         (modulo data-slug):\n--- md ---\n{footnotes_md}\n--- mdx ---\n{footnotes_mdx}"
    );

    // Structural assertions on the shared shape (first-reference numbering,
    // reference-order definitions, distinct occurrence ids for a repeated
    // reference, matching backrefs).
    assert!(
        footnotes_md.contains(
            "First footnote<sup><a href=\"#user-content-fn-a\" \
             id=\"user-content-fnref-a\" data-footnote-ref \
             aria-describedby=\"footnote-label\">1</a></sup>"
        ),
        "footnote `a`'s FIRST occurrence must be numbered 1: {footnotes_md}"
    );
    assert!(
        footnotes_md.contains(
            "a second<sup><a href=\"#user-content-fn-b\" \
             id=\"user-content-fnref-b\" data-footnote-ref \
             aria-describedby=\"footnote-label\">2</a></sup>"
        ),
        "footnote `b`, first referenced second, must be numbered 2 \
         (first-reference-order numbering): {footnotes_md}"
    );
    assert!(
        footnotes_md.contains(
            "the first<sup><a href=\"#user-content-fn-a\" \
             id=\"user-content-fnref-a-1\" data-footnote-ref \
             aria-describedby=\"footnote-label\">1</a></sup>"
        ),
        "footnote `a`'s REPEATED occurrence must share number 1 but mint a \
         distinct occurrence id (user-content-fnref-a-1): {footnotes_md}"
    );
    // Definitions render in reference order (a before b) — `a`'s <li> comes
    // first in the source string.
    let fn_a_li_pos = footnotes_md
        .find("<li id=\"user-content-fn-a\">")
        .expect("expected footnote `a` definition");
    let fn_b_li_pos = footnotes_md
        .find("<li id=\"user-content-fn-b\">")
        .expect("expected footnote `b` definition");
    assert!(
        fn_a_li_pos < fn_b_li_pos,
        "footnote definitions must render in reference order (a before b): {footnotes_md}"
    );
    // Both of `a`'s occurrences get their own backreference link, both
    // pointing back at `a`'s definition list item.
    assert!(
        footnotes_md.contains(
            "<a href=\"#user-content-fnref-a\" data-footnote-backref \
             aria-label=\"Back to reference 1\">"
        ),
        "expected a backref for `a`'s first occurrence: {footnotes_md}"
    );
    assert!(
        footnotes_md.contains(
            "<a href=\"#user-content-fnref-a-1\" data-footnote-backref \
             aria-label=\"Back to reference 1-2\">"
        ),
        "expected a distinct backref for `a`'s repeated occurrence: {footnotes_md}"
    );

    // --- Nested-in-JSX (site 3): semantic equivalence, not byte match ---
    let nested = read_page(&dist, "notes/nested");

    // The `<Note>` wrapper actually rendered (proves real MDX/JSX
    // evaluation of the component, not a dropped/fallback render).
    assert!(
        nested.contains("data-component=\"note\""),
        "expected the <Note> wrapper to render: {nested}"
    );

    // Task-list checked state, nested inside the JSX subtree, matches the
    // top-level fixture's semantics (unchecked then checked) — tolerating
    // the known nested-path whitespace difference (a literal space before
    // the following <p>) rather than forcing a byte match against the
    // top-level rendering.
    assert!(
        nested.contains("<input type=\"checkbox\" disabled/> <p>Nested task unchecked</p>"),
        "expected the unchecked nested task-list item: {nested}"
    );
    assert!(
        nested.contains("<input type=\"checkbox\" checked disabled/> <p>Nested task checked</p>"),
        "expected the checked nested task-list item: {nested}"
    );

    // Footnote reference nested inside <Note>'s children resolves to number
    // 1 and the SAME `user-content-fn-n` / `user-content-fnref-n` id
    // contract as the top-level path — tolerating the nested path's known
    // `data-footnote-ref="true"` (vs the top-level path's bare
    // `data-footnote-ref`) as a legitimate serialization difference between
    // the two emit sites, not a semantic one.
    assert!(
        nested.contains(
            "Ref inside note<sup><a href=\"#user-content-fn-n\" \
             id=\"user-content-fnref-n\" data-footnote-ref=\"true\" \
             aria-describedby=\"footnote-label\">1</a></sup>"
        ),
        "expected the nested footnote reference marker: {nested}"
    );

    // The footnote SECTION renders once, at the end of the WHOLE document —
    // i.e. AFTER the `<Note>` wrapper closes, never nested inside it. This
    // is the document-level footnote model's central contract: a reference
    // living inside a JSX subtree must not fragment or duplicate the
    // section, and the section must not itself get trapped inside the JSX
    // subtree it was referenced from.
    let note_close_pos = nested
        .find("</aside>")
        .expect("expected the <Note> wrapper to close");
    let section_pos = nested
        .find("<section data-footnotes")
        .expect("expected exactly one footnotes section");
    assert!(
        section_pos > note_close_pos,
        "the footnotes section must render AFTER </aside> closes, not nested \
         inside it: {nested}"
    );
    assert_eq!(
        nested.matches("<section data-footnotes").count(),
        1,
        "expected exactly one footnotes section (no duplication): {nested}"
    );
    assert!(
        nested.contains(
            "<li id=\"user-content-fn-n\"><p>Nested footnote body.</p>\
             <a href=\"#user-content-fnref-n\" data-footnote-backref \
             aria-label=\"Back to reference 1\">"
        ),
        "expected the nested footnote's definition + backref: {nested}"
    );
}
