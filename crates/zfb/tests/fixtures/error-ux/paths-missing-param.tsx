// Dynamic route page with a `paths()` export that returns the wrong
// param name — `wrongname` instead of `slug`. Triggers
// `PathsError::MissingParam` when expanded against the route template
// `blog/[slug].tsx`.
export const frontmatter = { title: "Bad paths" };

export function paths() {
  return [{ params: { wrongname: "hello" } }];
}

export default function Page() {
  return null;
}
