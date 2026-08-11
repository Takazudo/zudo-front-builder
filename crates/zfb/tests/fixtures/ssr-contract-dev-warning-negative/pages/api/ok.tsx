// Negative control (#2357, epic #2351): the CORRECT zero-parameter
// API-handler shape. `DefaultExportFirstParam::Absent` never reaches the
// detector's gate, so this must never warn.
export const prerender = false;

export default async function OkHandler() {
  return new Response("OK_NO_PARAM");
}
