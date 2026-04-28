// Page that intentionally throws at SSR time. The framed renderer is
// fed a synthetic source map (built in the test) that maps the bundled
// throw site back to this file.
export const frontmatter = { title: "Throws" };

export default function Page() {
  throw new Error("intentional SSR failure");
}
