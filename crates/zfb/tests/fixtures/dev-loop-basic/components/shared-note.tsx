/**
 * Shared component imported by BOTH pages (`pages/index.tsx` and
 * `pages/posts/[slug].tsx`) so the dev E2E harness can prove that a
 * `.tsx` module edit fans out to every route that imports it
 * (#1018, scenario 4 — the harness rewrites the marker below).
 */
export function SharedNote() {
  return <p data-testid="shared-note">SHARED-MARKER-V1</p>;
}
