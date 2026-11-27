export const prerender = false;

export default function Empty() {
  return new Response(null, { status: 204 });
}
