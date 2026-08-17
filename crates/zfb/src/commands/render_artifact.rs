//! Issue #2426 (epic #2421): the render-artifact export pass — extract
//! each page's content region, strip every sentinel, write the versioned
//! JSON artifact.
//!
//! ## Where this runs
//!
//! Between the link-base rewrite (step 3.6) and HTML minification
//! (step 3.7) in [`crate::commands::build`]. That placement is the whole
//! point of the epic: the fragment must carry the SAME asset URLs and
//! base-prefixed links the shipped page does (both rewrites have already
//! run), while every later pass — minification, island-marker check,
//! `postBuild` — must see marker-free HTML.
//!
//! ## The sentinel serialization (pinned by the epic)
//!
//! ```text
//! <template data-zfb-render-region="start" data-zfb-region-id="mdx://blog/hello"></template>
//! …region content…
//! <template data-zfb-render-region="end" data-zfb-region-id="mdx://blog/hello"></template>
//! ```
//!
//! The `data-zfb-render-region` / `data-zfb-region-id` attribute
//! namespace is **reserved** by zfb. Matching is nonetheless
//! EXACT-BYTE — every literal above, in that order, including the
//! double quotes and the empty element body. Anything that merely looks
//! like a marker (single quotes, an extra attribute, different case, a
//! self-closing spelling) is authored HTML and passes through
//! untouched. That is what lets the pass run over arbitrary user pages
//! without a parser: it removes only byte sequences it could have
//! written itself.
//!
//! ## Region rule (pinned by the epic)
//!
//! A route gets an artifact iff its page contains exactly one TOP-LEVEL
//! region. Zero regions → skipped silently (most pages are not
//! markdown-backed). Two or more sibling top-level regions (a listing
//! page rendering several entries) → one warning naming the route and
//! no artifact. Nested regions → the OUTER region is the route's region,
//! and the inner sentinels are stripped from the extracted fragment as
//! well as from the page.
//!
//! Sentinels are stripped from EVERY page whether or not an artifact was
//! written, and the stripped bytes equal the bytes the same build would
//! have produced with the markers never emitted — the markers introduce
//! no whitespace and no text nodes, so removing their byte ranges is the
//! exact inverse of emitting them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use zfb_content::mdx_jsx_emit::HeadingEntry;
use zfb_content::{region_id_without_hash, ContentSnapshot, RenderRegionMetadata};

use crate::output;

/// Version of the render-artifact JSON contract.
///
/// **Bump this whenever the emitted shape changes** — a field added,
/// removed, renamed, reordered, or given different semantics. Consumers
/// (zudo-doc-cloud's SPA Preview shell) branch on it, so a silent shape
/// change under an unchanged version is the one failure mode this
/// constant exists to prevent. Mirrors the `FINGERPRINT_VERSION`
/// discipline in `crates/zfb-content/src/pipeline.rs`.
pub(crate) const RENDER_ARTIFACT_CONTRACT_VERSION: u32 = 1;

/// Artifact directory relative to `outDir`. The `__zfb/` prefix is the
/// same framework-private namespace `__zfb/routes.json` uses.
const RENDER_ARTIFACT_REL_DIR: &str = "__zfb/render";

/// One route's render artifact, in the epic's pinned field order.
///
/// serde emits fields in declaration order, so the declaration order
/// here IS the on-disk order: `contractVersion`, `route`, `fragmentHtml`,
/// `headings`, `sourceDigest`. [`HeadingEntry`] likewise declares
/// `depth, text, slug` for this contract.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderArtifact<'a> {
    contract_version: u32,
    /// URL path of the route (`/blog/hello-world/`), not the output path.
    route: &'a str,
    /// The content region exactly as the shipped page carries it, minus
    /// the sentinels. With `minifyHtml` off this is a byte substring of
    /// the page written to `dist/`.
    fragment_html: &'a str,
    headings: &'a [HeadingEntry],
    source_digest: &'a str,
}

// ---------------------------------------------------------------------------
// Metadata index
// ---------------------------------------------------------------------------

/// Region id → `{ headings, sourceDigest }`, the join side of the
/// artifact writer.
///
/// Two spellings of the same region id are live in-tree — the Rust
/// snapshot's `module_specifier` carries the `#<hash8>` fragment while
/// `buildModuleSpecifier` in `packages/zfb/src/content.ts` mints the
/// hash-less form — so every entry is registered under both. A key that
/// two different entries answer to is marked AMBIGUOUS and resolves to
/// `None`: that collision is reachable (a specifier's `<collection>`
/// segment is the file's parent DIRECTORY name, so `guides/intro.md` in
/// two collections both mint `mdx://guides/intro`), and picking one
/// arbitrarily would write one region's headings into the other's
/// artifact. Same fail-closed rule as
/// [`ContentSnapshot::render_metadata_for`], but indexed rather than
/// linear — the writer resolves one id per route.
#[derive(Debug, Default)]
pub(crate) struct RenderMetadataIndex {
    entries: Vec<RenderRegionMetadata>,
    /// `None` marks an id more than one entry answers to.
    by_id: BTreeMap<String, Option<usize>>,
}

impl RenderMetadataIndex {
    /// Register one region's metadata under both spellings of its id.
    pub(crate) fn insert(&mut self, specifier: &str, metadata: RenderRegionMetadata) {
        let idx = self.entries.len();
        self.entries.push(metadata);
        self.register(specifier.to_string(), idx);
        let hashless = region_id_without_hash(specifier);
        if hashless != specifier {
            self.register(hashless.to_string(), idx);
        }
    }

    fn register(&mut self, key: String, idx: usize) {
        match self.by_id.get(&key) {
            None => {
                self.by_id.insert(key, Some(idx));
            }
            // Ambiguity is decided on PAYLOAD equality, not entry
            // identity: overlapping collection roots (a documented-legal
            // layout) walk the same file twice, inserting byte-identical
            // metadata under both spellings. Two entries answering to one
            // key with the SAME `{ headings, source_digest }` still
            // identify one region, so the key stays resolvable; only
            // genuinely divergent payloads fail closed.
            Some(Some(existing))
                if *existing == idx || self.entries[*existing] == self.entries[idx] => {}
            Some(Some(_)) => {
                self.by_id.insert(key, None);
            }
            Some(None) => {}
        }
    }

    /// Move every entry's render metadata OUT of `snapshot` and into this
    /// index.
    ///
    /// Draining rather than copying is load-bearing: the same
    /// `ContentSnapshot` is serialized into the worker bundle, and
    /// `EntrySnapshot::render_metadata` is `skip_serializing_if`-elided
    /// when `None`. Taking the metadata here leaves the snapshot in its
    /// exact flag-OFF serialized shape, so arming the channel cannot
    /// perturb the bundle bytes — and therefore cannot perturb rendered
    /// HTML, which is the epic's hard flag-on/flag-off invariant.
    pub(crate) fn drain_snapshot(&mut self, snapshot: &mut ContentSnapshot) {
        for entries in snapshot.collections.values_mut() {
            for entry in entries.iter_mut() {
                if let Some(metadata) = entry.render_metadata.take() {
                    let specifier = entry.module_specifier.clone();
                    self.insert(&specifier, metadata);
                }
            }
        }
    }

    /// Resolve a region id, accepting either spelling. `None` when
    /// nothing matches or when the id is ambiguous.
    pub(crate) fn get(&self, region_id: &str) -> Option<&RenderRegionMetadata> {
        self.by_id
            .get(region_id)
            .copied()
            .flatten()
            .map(|idx| &self.entries[idx])
    }
}

// ---------------------------------------------------------------------------
// Sentinel scanning
// ---------------------------------------------------------------------------

const MARKER_HEAD: &[u8] = b"<template data-zfb-render-region=\"";
const MARKER_KIND_START: &[u8] = b"start\"";
const MARKER_KIND_END: &[u8] = b"end\"";
const MARKER_ID_ATTR: &[u8] = b" data-zfb-region-id=\"";
/// Everything from the region id's closing quote to the end of the
/// element — an EMPTY `<template>`, which is what makes the marker inert
/// in the DOM and its removal byte-exact.
const MARKER_TAIL: &[u8] = b"\"></template>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Start,
    End,
}

struct Marker {
    kind: MarkerKind,
    region_id: String,
    /// Total byte length of the matched marker.
    len: usize,
}

/// Match the pinned serialization at the START of `bytes`, or `None`.
///
/// Byte-level matching is safe on UTF-8 input: every literal byte above
/// is ASCII, and ASCII bytes never occur inside a multi-byte UTF-8
/// sequence, so a match can never begin mid-character.
fn parse_marker(bytes: &[u8]) -> Option<Marker> {
    let rest = bytes.strip_prefix(MARKER_HEAD)?;
    let (kind, rest) = if let Some(r) = rest.strip_prefix(MARKER_KIND_START) {
        (MarkerKind::Start, r)
    } else {
        (MarkerKind::End, rest.strip_prefix(MARKER_KIND_END)?)
    };
    let rest = rest.strip_prefix(MARKER_ID_ATTR)?;
    let quote = rest.iter().position(|&b| b == b'"')?;
    let (id_bytes, tail) = rest.split_at(quote);
    if !tail.starts_with(MARKER_TAIL) {
        return None;
    }
    // The id is taken verbatim, whatever it contains. A specifier whose
    // slug carried an `&` would reach the page HTML-escaped
    // (`a&amp;b`), and the writer's join against the raw specifier would
    // then miss — but the marker is still RECOGNISED, so it is still
    // stripped. Rejecting escaped ids would invert that: an unjoinable
    // region would leak a live `<template data-zfb-render-region…>` into
    // shipped HTML, which is far worse than a missing artifact. An empty
    // id addresses nothing and is the one plausible authored placeholder,
    // so it stays rejected.
    if id_bytes.is_empty() {
        return None;
    }
    let region_id = std::str::from_utf8(id_bytes).ok()?.to_string();
    let len = MARKER_HEAD.len()
        + match kind {
            MarkerKind::Start => MARKER_KIND_START.len(),
            MarkerKind::End => MARKER_KIND_END.len(),
        }
        + MARKER_ID_ATTR.len()
        + id_bytes.len()
        + MARKER_TAIL.len();
    Some(Marker {
        kind,
        region_id,
        len,
    })
}

/// A region that closed at nesting depth 0, with its fragment's byte
/// range in the STRIPPED output.
#[derive(Debug, PartialEq, Eq)]
struct TopLevelRegion {
    region_id: String,
    start: usize,
    end: usize,
}

/// What the sentinel scan found on one page.
#[derive(Debug)]
struct PageScan {
    /// The page with every sentinel removed.
    stripped: Vec<u8>,
    /// How many sentinels were removed. Zero means the file on disk is
    /// already correct and is not rewritten.
    sentinels: usize,
    /// Top-level regions in document order — `Err` when the sentinels do
    /// not nest well-formedly.
    regions: Result<Vec<TopLevelRegion>, String>,
}

/// Strip every sentinel from `html` and report the top-level regions.
///
/// One pass does both jobs deliberately: a region's fragment is the
/// slice of the STRIPPED output between its start and end markers, so
/// the fragment is nested-sentinel-free by construction rather than by a
/// second cleanup pass that could disagree with the first.
fn scan_page(html: &[u8]) -> PageScan {
    let mut stripped = Vec::with_capacity(html.len());
    // (region id, offset into `stripped` where this region's fragment begins)
    let mut open: Vec<(String, usize)> = Vec::new();
    let mut top_level: Vec<TopLevelRegion> = Vec::new();
    let mut malformed: Option<String> = None;
    let mut sentinels = 0usize;

    let mut cursor = 0usize; // bytes before this are copied or dropped
    let mut probe = 0usize; // next index to look for `<` at
    while let Some(rel) = html[probe..].iter().position(|&b| b == b'<') {
        let at = probe + rel;
        let Some(marker) = parse_marker(&html[at..]) else {
            probe = at + 1;
            continue;
        };
        stripped.extend_from_slice(&html[cursor..at]);
        sentinels += 1;
        match marker.kind {
            MarkerKind::Start => open.push((marker.region_id, stripped.len())),
            MarkerKind::End => match open.pop() {
                Some((open_id, fragment_start)) if open_id == marker.region_id => {
                    if open.is_empty() {
                        top_level.push(TopLevelRegion {
                            region_id: open_id,
                            start: fragment_start,
                            end: stripped.len(),
                        });
                    }
                }
                Some((open_id, _)) => {
                    malformed.get_or_insert(format!(
                        "region `{}` was closed while `{open_id}` was still open",
                        marker.region_id
                    ));
                }
                None => {
                    malformed.get_or_insert(format!(
                        "region `{}` has an end marker with no matching start",
                        marker.region_id
                    ));
                }
            },
        }
        cursor = at + marker.len;
        probe = cursor;
    }
    stripped.extend_from_slice(&html[cursor..]);

    if let Some((unclosed, _)) = open.last() {
        malformed.get_or_insert(format!("region `{unclosed}` was never closed"));
    }

    PageScan {
        stripped,
        sentinels,
        regions: match malformed {
            Some(reason) => Err(reason),
            None => Ok(top_level),
        },
    }
}

// ---------------------------------------------------------------------------
// The build step
// ---------------------------------------------------------------------------

/// Map a page's `outDir`-relative HTML output path to its artifact
/// destination: `blog/hello/index.html` →
/// `<outdir>/__zfb/render/blog/hello/index.json`.
///
/// Returns `None` for a path that would escape the artifact directory
/// (`..` components) — unreachable for renderer-written pages, but the
/// destination is fed straight to a write, so it is checked rather than
/// assumed.
fn artifact_dest(outdir: &Path, page_rel: &Path) -> Option<PathBuf> {
    if page_rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(
        outdir
            .join(RENDER_ARTIFACT_REL_DIR)
            .join(page_rel)
            .with_extension("json"),
    )
}

/// Serialize one artifact to its on-disk bytes.
///
/// Pretty-printed with 2-space indents plus a trailing newline, matching
/// `emit_routes_manifest_file` — the artifact is a diffable build output
/// and a consumer diffing it across runs must see stable bytes. Every
/// input is either a constant or derived deterministically from the
/// page, so identical builds serialize identically.
fn serialize_artifact(
    route: &str,
    fragment_html: &str,
    metadata: &RenderRegionMetadata,
) -> Result<String> {
    let artifact = RenderArtifact {
        contract_version: RENDER_ARTIFACT_CONTRACT_VERSION,
        route,
        fragment_html,
        headings: &metadata.headings,
        source_digest: &metadata.source_digest,
    };
    let mut json =
        serde_json::to_string_pretty(&artifact).context("serialise render artifact to JSON")?;
    json.push('\n');
    Ok(json)
}

/// Run the render-artifact export over the post-processable pages.
///
/// `pages` is the same set the link-base rewrite and the minifier see —
/// renderer-written SSG files minus `.html`-passthrough pages. EVERY
/// page in the set is scanned and sentinel-stripped: marker emission
/// (`content.ts`'s `Content` wrap) is not extension-gated, so a non-HTML
/// SSG output (`sitemap.xml.tsx`, a feed route) that renders
/// `<entry.Content />` carries sentinels too, and leaving them in would
/// ship live `<template>` markup inside XML/JSON output. Artifacts (and
/// the region-rule warnings) are still HTML-only — there is no HTML page
/// to extract a fragment from otherwise. `routes_by_page` maps each
/// page's absolute path to its URL path; a page missing from it can
/// still be stripped but cannot get an artifact (the `route` field would
/// have nothing to carry).
///
/// Returns the number of artifacts written.
///
/// # Flag off
///
/// Returns immediately. Nothing is read, nothing is written — a
/// marker-free build pays the cost of one boolean test, and (because the
/// markers are only emitted when the same flag is on) a flag-off page on
/// disk is already in its final shape.
pub(crate) fn export_render_artifacts(
    enabled: bool,
    outdir: &Path,
    pages: &[PathBuf],
    routes_by_page: &BTreeMap<PathBuf, String>,
    metadata: &RenderMetadataIndex,
) -> Result<usize> {
    if !enabled {
        return Ok(0);
    }

    // Sorted so warnings and writes are emitted in one order regardless
    // of how the renderer happened to report its files. No extension
    // filter here — stripping must cover every renderer-written output
    // (see the doc comment above); the HTML-only gate applies to
    // artifact writing below.
    let mut targets: Vec<&PathBuf> = pages.iter().collect();
    targets.sort();
    targets.dedup();

    let mut written = 0usize;
    for page in targets {
        let bytes = std::fs::read(page).with_context(|| {
            format!(
                "render artifacts: failed to read {} for region extraction",
                page.display()
            )
        })?;
        let scan = scan_page(&bytes);

        // Strip first and unconditionally: a page whose regions are
        // unusable must still ship marker-free HTML.
        if scan.sentinels > 0 {
            zfb_build::atomic_write(page, &scan.stripped).with_context(|| {
                format!(
                    "render artifacts: failed to write sentinel-stripped {}",
                    page.display()
                )
            })?;
        }

        // Non-HTML outputs are stripped (above) but never produce an
        // artifact or a region-rule warning — there is no HTML page to
        // extract a fragment from.
        if !crate::commands::build::is_html_output_path(page) {
            continue;
        }

        let route = routes_by_page.get(page.as_path());
        let describe = || {
            route
                .map(String::as_str)
                .map_or_else(|| page.display().to_string(), str::to_string)
        };

        let regions = match &scan.regions {
            Ok(regions) => regions,
            Err(reason) => {
                output::warn(format!(
                    "render artifacts: {} has malformed content-region markers ({reason}); \
                     sentinels were stripped but no artifact was written",
                    describe()
                ));
                continue;
            }
        };

        let region = match regions.len() {
            0 => continue,
            1 => &regions[0],
            n => {
                output::warn(format!(
                    "render artifacts: {} renders {n} top-level content regions; \
                     an artifact is written only for a route with exactly one \
                     (see `emitRenderArtifacts`)",
                    describe()
                ));
                continue;
            }
        };

        let Some(route) = route else {
            output::warn(format!(
                "render artifacts: {} is not in the route table; skipping its artifact",
                page.display()
            ));
            continue;
        };
        let Some(dest) = page
            .strip_prefix(outdir)
            .ok()
            .and_then(|rel| artifact_dest(outdir, rel))
        else {
            output::warn(format!(
                "render artifacts: cannot derive an artifact path for {}; skipping",
                page.display()
            ));
            continue;
        };
        let Some(meta) = metadata.get(&region.region_id) else {
            output::warn(format!(
                "render artifacts: no headings/digest metadata for region `{}` on {route}; \
                 skipping its artifact",
                region.region_id
            ));
            continue;
        };
        let Ok(fragment) = std::str::from_utf8(&scan.stripped[region.start..region.end]) else {
            output::warn(format!(
                "render artifacts: the content region on {route} is not valid UTF-8; skipping"
            ));
            continue;
        };

        let json = serialize_artifact(route, fragment, meta)?;
        zfb_build::atomic_write_string(&dest, &json)
            .with_context(|| format!("failed to write {}", dest.display()))?;
        written += 1;
    }

    output::info(format!(
        "render artifacts: wrote {written} artifact{} to {RENDER_ARTIFACT_REL_DIR}/",
        if written == 1 { "" } else { "s" }
    ));
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn start(id: &str) -> String {
        format!(
            "<template data-zfb-render-region=\"start\" data-zfb-region-id=\"{id}\"></template>"
        )
    }

    fn end(id: &str) -> String {
        format!("<template data-zfb-render-region=\"end\" data-zfb-region-id=\"{id}\"></template>")
    }

    fn meta(slug: &str) -> RenderRegionMetadata {
        RenderRegionMetadata::new(
            vec![HeadingEntry {
                depth: 2,
                text: "Basic usage".to_string(),
                slug: slug.to_string(),
            }],
            format!("sha256:{}", "ab".repeat(32)),
        )
    }

    fn index_with(specifier: &str) -> RenderMetadataIndex {
        let mut index = RenderMetadataIndex::default();
        index.insert(specifier, meta("basic-usage"));
        index
    }

    /// Write `html` as `<outdir>/index.html` and return
    /// `(outdir, page path, route map)` for `/`.
    fn fixture(html: &str) -> (tempfile::TempDir, PathBuf, BTreeMap<PathBuf, String>) {
        let tmp = tempdir().unwrap();
        let page = tmp.path().join("index.html");
        fs::write(&page, html).unwrap();
        let mut routes = BTreeMap::new();
        routes.insert(page.clone(), "/".to_string());
        (tmp, page, routes)
    }

    // --- parse_marker: exact-byte matching --------------------------------

    #[test]
    fn parse_marker_accepts_the_pinned_serialization() {
        let bytes = start("mdx://blog/hello");
        let m = parse_marker(bytes.as_bytes()).expect("pinned start marker must parse");
        assert_eq!(m.kind, MarkerKind::Start);
        assert_eq!(m.region_id, "mdx://blog/hello");
        assert_eq!(m.len, bytes.len());

        let bytes = end("mdx://blog/hello#a1b2c3d4");
        let m = parse_marker(bytes.as_bytes()).expect("pinned end marker must parse");
        assert_eq!(m.kind, MarkerKind::End);
        assert_eq!(m.region_id, "mdx://blog/hello#a1b2c3d4");
        assert_eq!(m.len, bytes.len());
    }

    /// Authored HTML that merely resembles the serialization is NOT a
    /// marker. Each case deviates in exactly one way.
    #[test]
    fn parse_marker_rejects_authored_lookalikes() {
        let lookalikes = [
            // single quotes
            "<template data-zfb-render-region='start' data-zfb-region-id='mdx://a/b'></template>",
            // extra attribute before the close
            "<template data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://a/b\" class=\"x\"></template>",
            // attributes in the other order
            "<template data-zfb-region-id=\"mdx://a/b\" data-zfb-render-region=\"start\"></template>",
            // self-closing spelling
            "<template data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://a/b\"/>",
            // non-empty element body
            "<template data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://a/b\">x</template>",
            // different element
            "<div data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://a/b\"></div>",
            // capitalised tag name
            "<TEMPLATE data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://a/b\"></TEMPLATE>",
            // unknown region kind
            "<template data-zfb-render-region=\"middle\" data-zfb-region-id=\"mdx://a/b\"></template>",
            // empty region id
            "<template data-zfb-render-region=\"start\" data-zfb-region-id=\"\"></template>",
            // missing the id attribute entirely
            "<template data-zfb-render-region=\"start\"></template>",
            // extra whitespace
            "<template  data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://a/b\"></template>",
        ];
        for html in lookalikes {
            assert!(
                parse_marker(html.as_bytes()).is_none(),
                "must not be treated as a sentinel: {html}"
            );
        }
    }

    /// The whole-page consequence of the rule above: a page whose author
    /// wrote marker-shaped HTML round-trips byte-identically and yields
    /// no region.
    #[test]
    fn authored_lookalikes_are_left_alone_by_the_page_scan() {
        let html = "<main><template data-zfb-render-region='start' data-zfb-region-id='mdx://a/b'>\
                    </template><p>hi</p></main>";
        let scan = scan_page(html.as_bytes());
        assert_eq!(scan.sentinels, 0);
        assert_eq!(scan.stripped, html.as_bytes());
        assert_eq!(scan.regions.unwrap(), Vec::new());
    }

    // --- scan_page: the region rule ---------------------------------------

    #[test]
    fn single_region_yields_its_fragment_and_strips_to_the_unmarked_bytes() {
        let never_markered = "<html><body><main><h1>Hi</h1></main></body></html>";
        let markered = format!(
            "<html><body><main>{}<h1>Hi</h1>{}</main></body></html>",
            start("mdx://blog/hello"),
            end("mdx://blog/hello")
        );
        let scan = scan_page(markered.as_bytes());
        assert_eq!(scan.sentinels, 2);
        assert_eq!(
            scan.stripped,
            never_markered.as_bytes(),
            "stripped bytes must equal the bytes of a never-markered build"
        );
        let regions = scan.regions.unwrap();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].region_id, "mdx://blog/hello");
        assert_eq!(
            &scan.stripped[regions[0].start..regions[0].end],
            b"<h1>Hi</h1>"
        );
    }

    #[test]
    fn zero_regions_leaves_the_page_and_reports_nothing() {
        let html = "<html><body><p>plain</p></body></html>";
        let scan = scan_page(html.as_bytes());
        assert_eq!(scan.sentinels, 0);
        assert_eq!(scan.stripped, html.as_bytes());
        assert!(scan.regions.unwrap().is_empty());
    }

    #[test]
    fn sibling_top_level_regions_are_both_reported() {
        let markered = format!(
            "<ul>{}<li>a</li>{}{}<li>b</li>{}</ul>",
            start("mdx://blog/a"),
            end("mdx://blog/a"),
            start("mdx://blog/b"),
            end("mdx://blog/b")
        );
        let scan = scan_page(markered.as_bytes());
        assert_eq!(scan.stripped, b"<ul><li>a</li><li>b</li></ul>");
        let regions = scan.regions.unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].region_id, "mdx://blog/a");
        assert_eq!(regions[1].region_id, "mdx://blog/b");
    }

    /// A listing page can render the SAME entry twice. The pairs match by
    /// nesting order, so both siblings close correctly rather than the
    /// first `end` closing the outermost open frame.
    #[test]
    fn identical_id_siblings_pair_by_nesting_order() {
        let id = "mdx://blog/same";
        let markered = format!(
            "<ul>{}<li>one</li>{}{}<li>two</li>{}</ul>",
            start(id),
            end(id),
            start(id),
            end(id)
        );
        let scan = scan_page(markered.as_bytes());
        assert_eq!(scan.stripped, b"<ul><li>one</li><li>two</li></ul>");
        let regions = scan.regions.unwrap();
        assert_eq!(regions.len(), 2, "two sibling regions, not one nested pair");
        assert_eq!(
            &scan.stripped[regions[0].start..regions[0].end],
            b"<li>one</li>"
        );
        assert_eq!(
            &scan.stripped[regions[1].start..regions[1].end],
            b"<li>two</li>"
        );
    }

    /// Nested regions: the outer region is the route's region and the
    /// inner sentinels are gone from the extracted fragment too.
    #[test]
    fn nested_regions_report_only_the_outer_one_with_inner_sentinels_stripped() {
        let markered = format!(
            "<main>{}<p>outer</p>{}<em>inner</em>{}<p>tail</p>{}</main>",
            start("mdx://docs/outer"),
            start("mdx://docs/inner"),
            end("mdx://docs/inner"),
            end("mdx://docs/outer")
        );
        let scan = scan_page(markered.as_bytes());
        assert_eq!(scan.sentinels, 4);
        assert_eq!(
            scan.stripped,
            b"<main><p>outer</p><em>inner</em><p>tail</p></main>"
        );
        let regions = scan.regions.unwrap();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].region_id, "mdx://docs/outer");
        let fragment = &scan.stripped[regions[0].start..regions[0].end];
        assert_eq!(fragment, b"<p>outer</p><em>inner</em><p>tail</p>");
        assert!(
            !String::from_utf8_lossy(fragment).contains("data-zfb-render-region"),
            "no sentinel may survive inside the extracted fragment"
        );
    }

    /// Identical ids nested inside each other still pair innermost-first,
    /// so the OUTER pair is the one that reaches depth 0.
    #[test]
    fn identical_id_nesting_still_reports_the_outer_pair() {
        let id = "mdx://docs/same";
        let markered = format!(
            "<main>{}<p>a</p>{}<p>b</p>{}<p>c</p>{}</main>",
            start(id),
            start(id),
            end(id),
            end(id)
        );
        let scan = scan_page(markered.as_bytes());
        let regions = scan.regions.unwrap();
        assert_eq!(regions.len(), 1);
        assert_eq!(
            &scan.stripped[regions[0].start..regions[0].end],
            b"<p>a</p><p>b</p><p>c</p>"
        );
    }

    #[test]
    fn malformed_nesting_is_reported_but_still_strips_every_sentinel() {
        let unclosed = format!("<main>{}<p>x</p></main>", start("mdx://a/b"));
        let scan = scan_page(unclosed.as_bytes());
        assert_eq!(scan.stripped, b"<main><p>x</p></main>");
        assert!(scan.regions.unwrap_err().contains("never closed"));

        let orphan_end = format!("<main><p>x</p>{}</main>", end("mdx://a/b"));
        let scan = scan_page(orphan_end.as_bytes());
        assert_eq!(scan.stripped, b"<main><p>x</p></main>");
        assert!(scan.regions.unwrap_err().contains("no matching start"));

        let crossed = format!(
            "<main>{}{}{}{}</main>",
            start("mdx://a/one"),
            start("mdx://a/two"),
            end("mdx://a/one"),
            end("mdx://a/two")
        );
        let scan = scan_page(crossed.as_bytes());
        assert_eq!(scan.stripped, b"<main></main>");
        assert!(scan.regions.unwrap_err().contains("still open"));
    }

    /// The strip is a byte-range deletion, so multi-byte UTF-8 and
    /// `<`-heavy neighbouring markup around the markers survive intact.
    #[test]
    fn stripping_preserves_neighbouring_bytes_exactly() {
        let never_markered = "<p>日本語 <a href=\"/x?a=1&b=2\">リンク</a></p>";
        let markered = format!(
            "{}{never_markered}{}",
            start("mdx://ja/page"),
            end("mdx://ja/page")
        );
        let scan = scan_page(markered.as_bytes());
        assert_eq!(scan.stripped, never_markered.as_bytes());
        let regions = scan.regions.unwrap();
        assert_eq!(
            &scan.stripped[regions[0].start..regions[0].end],
            never_markered.as_bytes()
        );
    }

    // --- RenderMetadataIndex ----------------------------------------------

    #[test]
    fn index_resolves_both_spellings_of_a_region_id() {
        let index = index_with("mdx://docs/intro#a1b2c3d4");
        assert!(index.get("mdx://docs/intro#a1b2c3d4").is_some());
        assert!(index.get("mdx://docs/intro").is_some());
        assert!(index.get("mdx://docs/other").is_none());
        // A stale hash must never resolve against current metadata.
        assert!(index.get("mdx://docs/intro#99999999").is_none());
    }

    /// Overlapping collection roots walk the same file twice, inserting
    /// byte-identical metadata under identical specifiers. Identical
    /// payloads still identify ONE region, so the key must stay
    /// resolvable — only genuinely divergent payloads fail closed.
    #[test]
    fn identical_duplicate_inserts_do_not_poison_the_key() {
        let mut index = RenderMetadataIndex::default();
        index.insert("mdx://docs/intro#a1b2c3d4", meta("intro"));
        index.insert("mdx://docs/intro#a1b2c3d4", meta("intro"));
        assert!(
            index.get("mdx://docs/intro#a1b2c3d4").is_some(),
            "byte-identical duplicate metadata must not mark the key ambiguous"
        );
        assert!(index.get("mdx://docs/intro").is_some());
    }

    #[test]
    fn index_fails_closed_on_an_ambiguous_hashless_id() {
        let mut index = RenderMetadataIndex::default();
        index.insert("mdx://guides/intro#aaaaaaaa", meta("one"));
        index.insert("mdx://guides/intro#bbbbbbbb", meta("two"));
        assert!(
            index.get("mdx://guides/intro").is_none(),
            "a hash-less id two entries answer to must fail closed"
        );
        // Each hash-bearing spelling still resolves unambiguously.
        assert_eq!(
            index.get("mdx://guides/intro#aaaaaaaa").unwrap().headings[0].slug,
            "one"
        );
        assert_eq!(
            index.get("mdx://guides/intro#bbbbbbbb").unwrap().headings[0].slug,
            "two"
        );
    }

    /// Draining moves the metadata out, leaving the snapshot in the exact
    /// shape it serializes to with the flag off.
    #[test]
    fn draining_a_snapshot_leaves_it_serializing_as_flag_off() {
        use zfb_content::EntrySnapshot;

        let make = |render_metadata: Option<RenderRegionMetadata>| ContentSnapshot {
            collections: [(
                "docs".to_string(),
                vec![EntrySnapshot {
                    slug: "intro".to_string(),
                    frontmatter: serde_json::Value::Null,
                    body: "# Intro".to_string(),
                    module_specifier: "mdx://docs/intro#a1b2c3d4".to_string(),
                    rel_path: "intro.md".to_string(),
                    render_metadata,
                }],
            )]
            .into_iter()
            .collect(),
        };

        let mut armed = make(Some(meta("intro")));
        let flag_off = make(None);
        assert_ne!(
            serde_json::to_string(&armed).unwrap(),
            serde_json::to_string(&flag_off).unwrap(),
            "the armed snapshot must genuinely differ before draining"
        );

        let mut index = RenderMetadataIndex::default();
        index.drain_snapshot(&mut armed);

        assert_eq!(
            serde_json::to_string(&armed).unwrap(),
            serde_json::to_string(&flag_off).unwrap(),
            "draining must restore the flag-off serialized bytes exactly"
        );
        assert!(index.get("mdx://docs/intro").is_some());
    }

    // --- artifact path mapping --------------------------------------------

    #[test]
    fn artifact_dest_maps_html_output_paths_under_the_render_dir() {
        let outdir = Path::new("/dist");
        assert_eq!(
            artifact_dest(outdir, Path::new("blog/hello-world/index.html")).unwrap(),
            Path::new("/dist/__zfb/render/blog/hello-world/index.json")
        );
        assert_eq!(
            artifact_dest(outdir, Path::new("index.html")).unwrap(),
            Path::new("/dist/__zfb/render/index.json")
        );
        assert!(artifact_dest(outdir, Path::new("../escape/index.html")).is_none());
    }

    // --- export_render_artifacts (on-disk) --------------------------------

    #[test]
    fn flag_off_reads_nothing_and_writes_nothing() {
        let markered = format!(
            "<main>{}<h1>Hi</h1>{}</main>",
            start("mdx://blog/hello"),
            end("mdx://blog/hello")
        );
        let (tmp, page, routes) = fixture(&markered);
        let written = export_render_artifacts(
            false,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index_with("mdx://blog/hello"),
        )
        .unwrap();
        assert_eq!(written, 0);
        assert_eq!(
            fs::read_to_string(&page).unwrap(),
            markered,
            "flag off must not touch the page at all"
        );
        assert!(!tmp.path().join("__zfb/render").exists());
    }

    #[test]
    fn single_region_writes_the_pinned_artifact_and_strips_the_page() {
        let markered = format!(
            "<main>{}<h1 id=\"hello\">Hi</h1>{}</main>",
            start("mdx://blog/hello#a1b2c3d4"),
            end("mdx://blog/hello#a1b2c3d4")
        );
        let (tmp, page, mut routes) = fixture(&markered);
        routes.insert(page.clone(), "/blog/hello-world/".to_string());

        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index_with("mdx://blog/hello#a1b2c3d4"),
        )
        .unwrap();
        assert_eq!(written, 1);
        assert_eq!(
            fs::read_to_string(&page).unwrap(),
            "<main><h1 id=\"hello\">Hi</h1></main>"
        );

        let raw = fs::read_to_string(tmp.path().join("__zfb/render/index.json")).unwrap();
        assert!(raw.ends_with('\n'));
        // Field ORDER is part of the contract, so assert on the raw text
        // rather than only on the parsed object.
        let keys: Vec<&str> = raw
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.split('"').next())
            .collect();
        assert_eq!(
            keys,
            vec![
                "contractVersion",
                "route",
                "fragmentHtml",
                "headings",
                "depth",
                "text",
                "slug",
                "sourceDigest"
            ]
        );

        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["contractVersion"], 1);
        assert_eq!(parsed["route"], "/blog/hello-world/");
        assert_eq!(parsed["fragmentHtml"], "<h1 id=\"hello\">Hi</h1>");
        assert_eq!(parsed["headings"][0]["depth"], 2);
        assert_eq!(parsed["headings"][0]["slug"], "basic-usage");
        assert!(parsed["sourceDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    /// With `minifyHtml` off the fragment is an exact byte substring of
    /// the page as shipped — the epic's `fragmentHtml` claim.
    #[test]
    fn fragment_is_a_byte_substring_of_the_stripped_page() {
        let markered = format!(
            "<html><body><main>{}<h1>Hi</h1><p>x &amp; y</p>{}</main></body></html>",
            start("mdx://blog/hello"),
            end("mdx://blog/hello")
        );
        let (tmp, page, routes) = fixture(&markered);
        export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index_with("mdx://blog/hello"),
        )
        .unwrap();

        let shipped = fs::read_to_string(&page).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("__zfb/render/index.json")).unwrap(),
        )
        .unwrap();
        let fragment = parsed["fragmentHtml"].as_str().unwrap();
        assert!(
            shipped.contains(fragment),
            "fragment {fragment:?} must be a substring of the shipped page {shipped:?}"
        );
    }

    #[test]
    fn zero_regions_writes_no_artifact_and_leaves_the_page_byte_identical() {
        let html = "<html><body><p>plain</p></body></html>";
        let (tmp, page, routes) = fixture(html);
        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index_with("mdx://blog/hello"),
        )
        .unwrap();
        assert_eq!(written, 0);
        assert_eq!(fs::read_to_string(&page).unwrap(), html);
        assert!(!tmp.path().join("__zfb/render").exists());
    }

    #[test]
    fn multiple_sibling_regions_skip_the_artifact_but_still_strip() {
        let markered = format!(
            "<ul>{}<li>a</li>{}{}<li>b</li>{}</ul>",
            start("mdx://blog/a"),
            end("mdx://blog/a"),
            start("mdx://blog/b"),
            end("mdx://blog/b")
        );
        let (tmp, page, routes) = fixture(&markered);
        let mut index = RenderMetadataIndex::default();
        index.insert("mdx://blog/a", meta("a"));
        index.insert("mdx://blog/b", meta("b"));

        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index,
        )
        .unwrap();
        assert_eq!(written, 0);
        assert_eq!(
            fs::read_to_string(&page).unwrap(),
            "<ul><li>a</li><li>b</li></ul>",
            "a listing page still ships sentinel-free HTML"
        );
        assert!(!tmp.path().join("__zfb/render").exists());
    }

    #[test]
    fn nested_regions_write_the_outer_regions_artifact() {
        let markered = format!(
            "<main>{}<p>outer</p>{}<em>inner</em>{}{}</main>",
            start("mdx://docs/outer"),
            start("mdx://docs/inner"),
            end("mdx://docs/inner"),
            end("mdx://docs/outer")
        );
        let (tmp, page, routes) = fixture(&markered);
        let mut index = RenderMetadataIndex::default();
        index.insert("mdx://docs/outer", meta("outer"));
        index.insert("mdx://docs/inner", meta("inner"));

        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index,
        )
        .unwrap();
        assert_eq!(written, 1);
        let parsed: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("__zfb/render/index.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            parsed["headings"][0]["slug"], "outer",
            "the OUTER region's metadata is the route's"
        );
        assert_eq!(
            parsed["fragmentHtml"], "<p>outer</p><em>inner</em>",
            "inner sentinels must not survive in the fragment"
        );
    }

    /// The acceptance criterion in its literal form: run the pass over a
    /// markered page and over the page the same build would have written
    /// with the markers never emitted, then compare the two files byte
    /// for byte. It also pins that the never-markered page is untouched.
    #[test]
    fn stripped_page_bytes_equal_never_markered_page_bytes() {
        let never_markered = "<!doctype html><html lang=\"en\"><head><title>T</title></head>\
                              <body><main><h1 id=\"hi\">Hi</h1><p>a &amp; b</p></main>\
                              <script type=\"module\" src=\"/assets/x-9f8e7d.js\"></script>\
                              </body></html>";
        let markered = never_markered.replace(
            "<main>",
            &format!("<main>{}", start("mdx://blog/hello#a1b2c3d4")),
        );
        let markered = markered.replace(
            "</main>",
            &format!("{}</main>", end("mdx://blog/hello#a1b2c3d4")),
        );
        assert_ne!(markered, never_markered, "the fixture must differ up front");

        let tmp = tempdir().unwrap();
        let with = tmp.path().join("with/index.html");
        let without = tmp.path().join("without/index.html");
        fs::create_dir_all(with.parent().unwrap()).unwrap();
        fs::create_dir_all(without.parent().unwrap()).unwrap();
        fs::write(&with, &markered).unwrap();
        fs::write(&without, never_markered).unwrap();

        let mut routes = BTreeMap::new();
        routes.insert(with.clone(), "/with/".to_string());
        routes.insert(without.clone(), "/without/".to_string());

        export_render_artifacts(
            true,
            tmp.path(),
            &[with.clone(), without.clone()],
            &routes,
            &index_with("mdx://blog/hello#a1b2c3d4"),
        )
        .unwrap();

        assert_eq!(
            fs::read(&with).unwrap(),
            fs::read(&without).unwrap(),
            "a stripped page must be byte-identical to the never-markered build"
        );
        assert_eq!(fs::read_to_string(&without).unwrap(), never_markered);
    }

    /// A marker whose id cannot be joined (here: HTML-escaped by the
    /// attribute serializer, so it does not match any indexed specifier)
    /// must STILL be stripped. Leaking a live sentinel into shipped HTML
    /// is a worse failure than a missing artifact, so recognition is
    /// deliberately wider than the join.
    #[test]
    fn an_unjoinable_region_id_is_still_stripped_from_the_page() {
        let escaped = "mdx://pages/a&amp;b";
        let markered = format!("<main>{}<h1>Hi</h1>{}</main>", start(escaped), end(escaped));
        let (tmp, page, routes) = fixture(&markered);
        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &index_with("mdx://pages/a&b"),
        )
        .unwrap();
        assert_eq!(written, 0, "the join misses, so no artifact is written");
        assert_eq!(
            fs::read_to_string(&page).unwrap(),
            "<main><h1>Hi</h1></main>",
            "the sentinel must not survive into shipped HTML"
        );
    }

    #[test]
    fn a_region_with_no_metadata_is_skipped_rather_than_written_incomplete() {
        let markered = format!(
            "<main>{}<h1>Hi</h1>{}</main>",
            start("mdx://blog/unknown"),
            end("mdx://blog/unknown")
        );
        let (tmp, page, routes) = fixture(&markered);
        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &RenderMetadataIndex::default(),
        )
        .unwrap();
        assert_eq!(written, 0);
        assert_eq!(
            fs::read_to_string(&page).unwrap(),
            "<main><h1>Hi</h1></main>",
            "the page is stripped even when the join misses"
        );
    }

    /// Non-HTML SSG outputs (XML feeds, JSON) are stripped — marker
    /// emission is not extension-gated, so a feed route rendering
    /// `<entry.Content />` carries sentinels too — but they never get an
    /// artifact and never trigger a region-rule warning.
    #[test]
    fn non_html_outputs_are_stripped_but_never_get_artifacts() {
        let tmp = tempdir().unwrap();
        let feed = tmp.path().join("feed.xml");
        let body = format!(
            "<feed>{}<item>hi</item>{}</feed>",
            start("mdx://blog/hello"),
            end("mdx://blog/hello")
        );
        fs::write(&feed, &body).unwrap();
        let written = export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&feed),
            &BTreeMap::new(),
            &index_with("mdx://blog/hello"),
        )
        .unwrap();
        assert_eq!(written, 0, "a non-HTML output must never get an artifact");
        assert_eq!(
            fs::read_to_string(&feed).unwrap(),
            "<feed><item>hi</item></feed>",
            "sentinels must not survive in a shipped non-HTML output"
        );
        assert!(!tmp.path().join("__zfb/render").exists());
    }

    /// Byte-stability across runs, mirroring
    /// `emit_routes_manifest_is_byte_stable_across_runs`: two identical
    /// builds must produce identical artifact bytes, or consumers that
    /// diff the file across runs see phantom churn.
    #[test]
    fn render_artifact_is_byte_stable_across_runs() {
        let markered = format!(
            "<main>{}<h1>Hi</h1>{}</main>",
            start("mdx://blog/hello"),
            end("mdx://blog/hello")
        );
        let run = || {
            let (tmp, page, routes) = fixture(&markered);
            export_render_artifacts(
                true,
                tmp.path(),
                &[page],
                &routes,
                &index_with("mdx://blog/hello"),
            )
            .unwrap();
            fs::read(tmp.path().join("__zfb/render/index.json")).unwrap()
        };
        let a = run();
        let b = run();
        assert_eq!(a, b, "render artifacts must be byte-stable across runs");
        assert!(a.ends_with(b"\n"));
    }

    /// A build with the flag on but no markers anywhere must leave every
    /// page byte-identical to the flag-off build — the epic's hard
    /// invariant, at the level this pass can prove it.
    #[test]
    fn pages_without_sentinels_are_never_rewritten() {
        let html = "<html><body><p>plain</p></body></html>";
        let (tmp, page, routes) = fixture(html);
        let before = fs::metadata(&page).unwrap().len();
        export_render_artifacts(
            true,
            tmp.path(),
            std::slice::from_ref(&page),
            &routes,
            &RenderMetadataIndex::default(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&page).unwrap(), html);
        assert_eq!(fs::metadata(&page).unwrap().len(), before);
    }
}
