// The exact broken shape #2350 reported: a `prerender = false` route whose
// default export's first parameter is annotated `Request`. zfb calls a
// page's default export with the page's props object, never the incoming
// Request, so `request.method` is `undefined` here and the handler 405s on
// every real request while `tsc --noEmit` sees nothing wrong.
export const frontmatter = { title: "Submit" };
export const prerender = false;
export default async function Handler(request: Request): Promise<Response> {
  return new Response("ok");
}
