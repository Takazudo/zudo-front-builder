//! Fixture generator. Emits the spike's TSX fixture surface under
//! `target/spike-fixtures/`:
//!
//! - 80 static TSX pages (`pages/static/page-NN.tsx`)
//! - 10 dynamic-route pages (`pages/dynamic/[slug-NN].tsx`)
//! - 10 collection-importing pages (`pages/collections/post-NN.tsx`)
//! - 3 shared components (`components/shared-NN.tsx`, one with `"use client"`)
//! - 1 top-level-await page (`pages/tla/late.tsx`)
//!
//! It also emits a parallel `bench-js/` directory containing pre-shaped ESM
//! modules that the bench binary feeds to each `RenderHost`. The bench
//! evaluates the `bench-js/` versions because the spike measures the JS
//! runtime, not the SWC TSX-to-JS step (that's a separate measurement axis
//! in ADR-001 and is the same pipeline regardless of runtime choice).

use std::fs;
use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let out = std::env::var_os("ZFB_SPIKE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/spike-fixtures"));
    fs::create_dir_all(&out)?;

    write_static_tsx(&out)?;
    write_dynamic_tsx(&out)?;
    write_collection_tsx(&out)?;
    write_shared_components(&out)?;
    write_tla_page(&out)?;
    write_bench_js(&out)?;

    println!("wrote fixtures into {}", out.display());
    Ok(())
}

fn write_static_tsx(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("pages").join("static");
    fs::create_dir_all(&dir)?;
    for n in 1..=80u32 {
        let body = format!(
            "import {{ Layout }} from \"../../components/shared-1\";\n\
             import {{ Card }} from \"../../components/shared-2\";\n\n\
             export const meta = {{ layout: \"default\", title: \"Page {n}\" }};\n\n\
             export default function Page() {{\n  return (\n    <Layout>\n\
             \u{20}     <h1>Static page {n}</h1>\n\
             \u{20}     <Card>Body for page {n}.</Card>\n    </Layout>\n  );\n}}\n"
        );
        fs::write(dir.join(format!("page-{:02}.tsx", n)), body)?;
    }
    Ok(())
}

fn write_dynamic_tsx(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("pages").join("dynamic");
    fs::create_dir_all(&dir)?;
    for n in 1..=10u32 {
        let body = format!(
            "import {{ Layout }} from \"../../components/shared-1\";\n\n\
             export async function paths() {{\n\
             \u{20} return [\n\
             \u{20}   {{ params: {{ slug: \"alpha-{n}\" }}, props: {{ idx: {n} }} }},\n\
             \u{20}   {{ params: {{ slug: \"beta-{n}\"  }}, props: {{ idx: {n} }} }},\n\
             \u{20} ];\n}}\n\n\
             export const meta = {{ layout: \"default\", title: \"Dyn {n}\" }};\n\n\
             export default function DynPage(props) {{\n  return (\n    <Layout>\n\
             \u{20}     <h1>Dynamic page {n}</h1>\n\
             \u{20}     <p>idx: {{props.idx}}</p>\n    </Layout>\n  );\n}}\n"
        );
        fs::write(dir.join(format!("[slug-{:02}].tsx", n)), body)?;
    }
    Ok(())
}

fn write_collection_tsx(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("pages").join("collections");
    fs::create_dir_all(&dir)?;
    for n in 1..=10u32 {
        let body = format!(
            "import {{ getCollection }} from \"zfb\";\n\
             import {{ Layout }} from \"../../components/shared-1\";\n\
             import {{ Card }} from \"../../components/shared-2\";\n\
             import {{ List }} from \"../../components/shared-3\";\n\n\
             export const meta = {{ layout: \"default\", title: \"Collection {n}\" }};\n\n\
             export default async function Page() {{\n\
             \u{20} const posts = await getCollection(\"posts\");\n\
             \u{20} const docs = await getCollection(\"docs\");\n\
             \u{20} return (\n    <Layout>\n\
             \u{20}     <h1>Posts ({n})</h1>\n\
             \u{20}     <List items={{posts}} />\n\
             \u{20}     <List items={{docs}} />\n    </Layout>\n  );\n}}\n"
        );
        fs::write(dir.join(format!("post-{:02}.tsx", n)), body)?;
    }
    Ok(())
}

fn write_shared_components(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("components");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("shared-1.tsx"),
        "export function Layout({ children }) {\n  return <main>{children}</main>;\n}\n",
    )?;
    fs::write(
        dir.join("shared-2.tsx"),
        "export function Card({ children }) {\n  return <section class=\"card\">{children}</section>;\n}\n",
    )?;
    fs::write(
        dir.join("shared-3.tsx"),
        "\"use client\";\nexport function List({ items }) {\n  return <ul>{items.map((i) => <li>{i.id}</li>)}</ul>;\n}\n",
    )?;
    Ok(())
}

fn write_tla_page(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("pages").join("tla");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("late.tsx"),
        "import { Layout } from \"../../components/shared-1\";\n\n\
         // top-level await — must succeed under the chosen runtime\n\
         const cfg = await fetch(\"https://stub.invalid/cfg.json\")\n\
         \u{20} .then((r) => r.json())\n\
         \u{20} .catch(() => ({ fallback: true }));\n\n\
         export const meta = { layout: \"default\", title: \"TLA\" };\n\n\
         export default function Page() {\n  return (\n    <Layout>\n\
         \u{20}     <pre>{JSON.stringify(cfg)}</pre>\n    </Layout>\n  );\n}\n",
    )?;
    Ok(())
}

/// Pre-shaped ESM the bench harness actually evaluates. These mirror the TSX
/// scenarios but with hand-written `h(...)`/string output so we don't need to
/// run SWC inside the spike — the goal is to measure the JS runtime, not the
/// transpile step.
fn write_bench_js(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("bench-js");
    fs::create_dir_all(&dir)?;

    // The bench-js fixtures use a "script with a top-level renderHTML()
    // function" shape. This keeps the bench engine-portable: rquickjs
    // doesn't need the async ESM eval path, and deno_core / ssr_rs each
    // also evaluate scripts that define `renderHTML` on the local scope.
    // The host wraps each fixture in an IIFE that returns `renderHTML`.

    fs::write(
        dir.join("static.mjs"),
        r#"// Mirrors a static TSX page after SWC + a stub render-to-string.
function h(tag, attrs) {
  var a = "";
  if (attrs) for (var k in attrs) a += " " + k + "=\"" + attrs[k] + "\"";
  var kids = "";
  for (var i = 2; i < arguments.length; i++) kids += arguments[i];
  return "<" + tag + a + ">" + kids + "</" + tag + ">";
}
function Layout(children) { return h("main", null, children); }
function Card(children) { return h("section", { class: "card" }, children); }
function renderHTML() {
  return Layout(h("h1", null, "Static page") + Card("Body for static page."));
}
"#,
    )?;

    fs::write(
        dir.join("dynamic.mjs"),
        r#"function h(tag, attrs) {
  var a = "";
  if (attrs) for (var k in attrs) a += " " + k + "=\"" + attrs[k] + "\"";
  var kids = "";
  for (var i = 2; i < arguments.length; i++) kids += arguments[i];
  return "<" + tag + a + ">" + kids + "</" + tag + ">";
}
function paths() {
  return [
    { params: { slug: "alpha" }, props: { idx: 1 } },
    { params: { slug: "beta"  }, props: { idx: 2 } }
  ];
}
function renderHTML() {
  var props = { idx: 7 };
  return h("main", null,
    h("h1", null, "Dynamic page") +
    h("p", null, "idx: " + props.idx));
}
"#,
    )?;

    fs::write(
        dir.join("tla.mjs"),
        r#"// Top-level-await scenario flattened to a sync resolution. The qualitative
// TLA axis is evaluated against the runtime's documented support, not the
// micro-bench loop, since each runtime drives microtasks differently.
var cfg = { stubbed: true, n: 42 };
function renderHTML() {
  return "<main><pre>" + JSON.stringify(cfg) + "</pre></main>";
}
"#,
    )?;

    fs::write(
        dir.join("use_client.mjs"),
        r#"// "use client" components render server-side identically to plain ones —
// the directive is a bundler marker. We model that here by ignoring it.
function h(tag, attrs) {
  var a = "";
  if (attrs) for (var k in attrs) a += " " + k + "=\"" + attrs[k] + "\"";
  var kids = "";
  for (var i = 2; i < arguments.length; i++) kids += arguments[i];
  return "<" + tag + a + ">" + kids + "</" + tag + ">";
}
function List(items) {
  var lis = "";
  for (var i = 0; i < items.length; i++) lis += h("li", null, items[i].id);
  return h("ul", null, lis);
}
function renderHTML() {
  var items = [{ id: "a" }, { id: "b" }, { id: "c" }];
  return h("main", null, h("h2", null, "List") + List(items));
}
"#,
    )?;

    fs::write(
        dir.join("collection.mjs"),
        r#"// Simulates a page that pulled two content collections. Synthesizing
// the data deterministically so the runtime is what we measure.
var fakePosts = [];
for (var i = 0; i < 12; i++) fakePosts.push({ id: "p-" + i });
var fakeDocs = [];
for (var j = 0; j < 8; j++) fakeDocs.push({ id: "d-" + j });
function h(tag, attrs) {
  var a = "";
  if (attrs) for (var k in attrs) a += " " + k + "=\"" + attrs[k] + "\"";
  var kids = "";
  for (var i = 2; i < arguments.length; i++) kids += arguments[i];
  return "<" + tag + a + ">" + kids + "</" + tag + ">";
}
function List(items) {
  var lis = "";
  for (var i = 0; i < items.length; i++) lis += h("li", null, items[i].id);
  return h("ul", null, lis);
}
function renderHTML() {
  return h("main", null,
    h("h1", null, "Collection page") +
    List(fakePosts) +
    List(fakeDocs));
}
"#,
    )?;

    Ok(())
}
