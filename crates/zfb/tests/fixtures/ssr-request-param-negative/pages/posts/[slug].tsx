// Negative control (b): a `prerender = false` DYNAMIC route whose handler
// destructures its first parameter (`{ params }`). A destructuring pattern
// fails the gate's "plain binding identifier" precondition, so this must
// never fire the detector either.
export const frontmatter = { title: "Post" };
export const prerender = false;
export default function Handler({ params }: { params: Record<string, string> }) {
  return new Response(`post: ${params.slug}`);
}
