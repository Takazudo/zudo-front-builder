/**
 * Zero-`paths()` sibling route (issue #2064, fixed on `main` as of
 * `6296428d`): a dynamic page module the router scan lists regardless of
 * what its `paths()` returns. Returning `[]` here expands into no route
 * entries at all — a tolerated `page_sources` member post-#2064 (see
 * `collect_page_sources_lists_a_dynamic_page_the_route_tables_may_never_hold`
 * in `crates/zfb/src/commands/dev.rs`), not the pre-fix
 * "content provenance unavailable" boot warning. Present in this fixture
 * so the #2063 reload repro exercises that shape alongside the ordinary
 * prerendered `posts` collection, not just a single-route project.
 */
export function paths() {
  return [];
}

export default function ArchiveYearPage() {
  return null;
}
