// `htmlPage(body, init?)` — an HTML page result with its own status and
// headers (#4887). Server-only: exported from `@takazudo/zfb-runtime/server`,
// never from the client-safe root barrel.
//
// A page's default export returns the descriptor instead of a raw `Response`
// when it needs a status or headers but still wants the router to manage the
// HTML: render JSX, add the doctype, and finish the build's asset URLs. A raw
// `Response` stays a passthrough whose body the router never reads.

// `Symbol.for` so two copies of this module (for example a duplicated
// dependency in one bundle) still recognise each other's descriptors.
const HTML_PAGE_BRAND: unique symbol = Symbol.for("@takazudo/zfb-runtime/html-page-v1");

const DEFAULT_HTML_CONTENT_TYPE = "text/html; charset=utf-8";

// The router rewrites the body (doctype, asset URLs), so any header that
// describes the original body bytes would be wrong on the response.
const BODY_DERIVED_HEADERS = [
  "content-length",
  "etag",
  "digest",
  "content-digest",
  "repr-digest",
  "content-md5",
] as const;

/** Statuses whose responses carry no body; return a raw `Response` for them. */
const BODYLESS_STATUSES: ReadonlySet<number> = new Set([204, 205, 304]);

// `reason-phrase` in RFC 9112 section 4, checked here because the embedded
// dev runtime's `Response` does not validate it.
const REASON_PHRASE = /^[\t\x20-\x7e\x80-\xff]*$/;

function isNonMarkupBody(body: unknown): boolean {
  if (typeof body !== "object" || body === null) return false;
  if (typeof (body as { then?: unknown }).then === "function") return true;
  if (ArrayBuffer.isView(body) || body instanceof ArrayBuffer) return true;
  if (typeof ReadableStream !== "undefined" && body instanceof ReadableStream) return true;
  if (typeof Blob !== "undefined" && body instanceof Blob) return true;
  return false;
}

/** Options accepted by [`htmlPage`]. */
export interface HtmlPageInit {
  /** HTTP status, an integer in 200..599 other than 204, 205 and 304. Default 200. */
  readonly status?: number;
  readonly statusText?: string;
  /**
   * Response headers. `Content-Type` defaults to `text/html; charset=utf-8`
   * and must stay `text/html`. `Content-Encoding` must be absent or
   * `identity`. Headers that describe the body bytes (`Content-Length`,
   * `ETag`, `Digest`, `Content-Digest`, `Repr-Digest`, `Content-MD5`) are
   * dropped because the router rewrites the body.
   */
  readonly headers?: HeadersInit;
}

/** The frozen descriptor [`htmlPage`] returns. Not a `Response`. */
export interface HtmlPageResult {
  readonly [HTML_PAGE_BRAND]: true;
  /** JSX to render, or an HTML string used as-is. */
  readonly body: unknown;
  readonly status: number;
  readonly statusText: string | undefined;
  /** Header pairs in order; each `Set-Cookie` value is its own pair. */
  readonly headers: readonly (readonly [name: string, value: string])[];
}

/**
 * Describe an HTML page with an explicit status and headers. Return it from a
 * page module's default export, typically for a request-time SSR route:
 *
 * ```tsx
 * return htmlPage(<NotFound />, { status: 404, headers: { "Cache-Control": "no-store" } });
 * ```
 *
 * The router renders `body` (a string as-is, JSX through the server
 * renderer), adds the HTML5 doctype when needed, finishes the build's asset
 * URLs, then builds the `Response`. Invalid options throw `TypeError` here,
 * at call time.
 */
export function htmlPage(body: unknown, init: HtmlPageInit = {}): HtmlPageResult {
  if (body instanceof Response) {
    throw new TypeError(
      "[zfb-runtime] htmlPage: body must be JSX or an HTML string, not a Response; return the Response itself instead",
    );
  }
  if (isHtmlPage(body)) {
    throw new TypeError("[zfb-runtime] htmlPage: body is already an htmlPage(...) result");
  }
  if (isNonMarkupBody(body)) {
    throw new TypeError(
      "[zfb-runtime] htmlPage: body must be JSX or an HTML string; return a raw Response for streams, binary data or promises",
    );
  }

  const status = init.status ?? 200;
  if (!Number.isInteger(status) || status < 200 || status > 599) {
    throw new TypeError(
      `[zfb-runtime] htmlPage: status must be an integer from 200 to 599, got ${String(status)}`,
    );
  }
  if (BODYLESS_STATUSES.has(status)) {
    throw new TypeError(
      `[zfb-runtime] htmlPage: status ${status} has no body; return a raw Response (for example new Response(null, { status: ${status} })) instead`,
    );
  }

  const statusText = init.statusText;
  if (statusText !== undefined) {
    if (typeof statusText !== "string" || !REASON_PHRASE.test(statusText)) {
      throw new TypeError(
        `[zfb-runtime] htmlPage: statusText must be a valid HTTP reason phrase, got ${JSON.stringify(statusText)}`,
      );
    }
  }

  const headers = new Headers(init.headers);
  const contentType = headers.get("content-type");
  if (contentType === null) {
    headers.set("content-type", DEFAULT_HTML_CONTENT_TYPE);
  } else if (mediaType(contentType) !== "text/html") {
    throw new TypeError(
      `[zfb-runtime] htmlPage: Content-Type must be text/html, got "${contentType}"; return a raw Response for other content`,
    );
  }
  const contentEncoding = headers.get("content-encoding");
  if (contentEncoding !== null) {
    if (contentEncoding.trim().toLowerCase() !== "identity") {
      throw new TypeError(
        `[zfb-runtime] htmlPage: Content-Encoding "${contentEncoding}" is not supported because the router rewrites the body; return a raw Response for encoded content`,
      );
    }
    headers.delete("content-encoding");
  }
  for (const name of BODY_DERIVED_HEADERS) headers.delete(name);

  const pairs: (readonly [string, string])[] = [];
  for (const [name, value] of headers) {
    if (name !== "set-cookie") pairs.push(Object.freeze([name, value] as const));
  }
  for (const cookie of headers.getSetCookie()) {
    pairs.push(Object.freeze(["set-cookie", cookie] as const));
  }

  return Object.freeze({
    [HTML_PAGE_BRAND]: true as const,
    body,
    status,
    statusText,
    headers: Object.freeze(pairs),
  });
}

/** `true` when `value` is a descriptor from [`htmlPage`] (any module copy). */
export function isHtmlPage(value: unknown): value is HtmlPageResult {
  return (
    typeof value === "object" &&
    value !== null &&
    (value as Record<symbol, unknown>)[HTML_PAGE_BRAND] === true
  );
}

/** Build the `Response` for a descriptor whose body is already final HTML. */
export function htmlPageResponse(page: HtmlPageResult, html: string): Response {
  const headers = new Headers();
  for (const [name, value] of page.headers) headers.append(name, value);
  return new Response(html, {
    status: page.status,
    headers,
    ...(page.statusText !== undefined && { statusText: page.statusText }),
  });
}

/** Lower-cased media type of a Content-Type value (the part before `;`). */
export function mediaType(contentType: string): string {
  return (contentType.split(";")[0] ?? "").trim().toLowerCase();
}
