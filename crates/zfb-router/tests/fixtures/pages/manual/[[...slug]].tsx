// Optional catchall fixture (#812): serves the bare `/manual` URL
// (slug = []) and every nested path (`/manual/a/b`, slug = ["a", "b"]).
export function paths() {
  return [{ params: { slug: [] } }, { params: { slug: ["setup", "quick"] } }];
}

export default function ManualPage() {
  return null;
}
