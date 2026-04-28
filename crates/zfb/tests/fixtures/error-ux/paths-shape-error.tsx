// `paths()` returns an object instead of the required array. Triggers
// `PathsError::InvalidPathsExport` ("got object", expected array).
export const frontmatter = { title: "Bad paths" };

export function paths() {
  return { not: "an array" };
}

export default function Page() {
  return null;
}
