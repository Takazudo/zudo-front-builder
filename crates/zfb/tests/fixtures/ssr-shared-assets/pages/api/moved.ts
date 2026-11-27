export const prerender = false;

export default function Moved() {
  return new Response(null, { status: 302, headers: { Location: "/live" } });
}
