// Negative control (a): the correct API-route shape for a `prerender =
// false` route — zero parameters, returns a `Response`. Must never fire the
// detector's gate.
export const frontmatter = { title: "Ping" };
export const prerender = false;
export default function Handler() {
  return new Response("ok");
}
