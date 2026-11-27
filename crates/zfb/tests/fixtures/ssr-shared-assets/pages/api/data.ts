export const prerender = false;

export default function Data() {
  return Response.json({ stylesheet: "/assets/styles.css" });
}
