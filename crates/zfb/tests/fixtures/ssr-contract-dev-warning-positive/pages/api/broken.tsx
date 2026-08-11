// Positive fixture (#2357, epic #2351): the silent `(request: Request)`
// SSR handler shape. `request: Request` type-checks fine — `Request` is a
// valid annotation for a parameter that actually receives the page's
// props object — but zfb calls a page's default export with the props
// object, never the incoming Request, so `request.method` is always
// `undefined` and this handler 405s on every real request. The
// detector's Strong tier (annotated `Request`, plain binding,
// `prerender = false`) must fire on exactly this shape.
export const prerender = false;

export default async function BrokenHandler(request: Request) {
  if (request.method !== "POST") {
    return new Response("BROKEN_405", { status: 405 });
  }
  return new Response("BROKEN_OK");
}
