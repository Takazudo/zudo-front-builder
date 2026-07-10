import { parseCookieHeader, serializeCookie } from "./cookies";

const AUTH_PATH = "/__auth";
const COOKIE_NAME = "zfb_preview_gate";
const AUTH_MARKER = "pg_01_hL7G9sR4vK2pQ8mN6bD3xA";
const COOKIE_MAX_AGE_SECONDS = 60 * 60 * 24 * 365;
const DEV_PASSWORD = "preview-open-sesame";

// SITE_PASSWORD is a secret, not a wrangler.toml var, so keep it out of config.
type RuntimeEnv = Env & {
  SITE_PASSWORD?: string;
};

export default {
  async fetch(request: Request, env: RuntimeEnv): Promise<Response> {
    const url = new URL(request.url);

    if (hasValidMarker(request)) {
      return env.ASSETS.fetch(request);
    }

    if (url.pathname === AUTH_PATH && request.method === "POST") {
      return handleAuth(request, env);
    }

    return loginResponse(sanitizeNext(url.pathname + url.search));
  },
} satisfies ExportedHandler<RuntimeEnv>;

function hasValidMarker(request: Request): boolean {
  const cookies = parseCookieHeader(request.headers.get("Cookie"));
  return cookies.get(COOKIE_NAME) === AUTH_MARKER;
}

async function handleAuth(request: Request, env: RuntimeEnv): Promise<Response> {
  const form = await request.formData();
  const password = form.get("password");
  const nextValue = form.get("next");
  const next = sanitizeNext(typeof nextValue === "string" ? nextValue : "/");
  const expectedPassword = env.SITE_PASSWORD || DEV_PASSWORD;

  if (typeof password !== "string" || !(await timingSafeEqual(password, expectedPassword))) {
    return loginResponse(next, true);
  }

  const url = new URL(request.url);
  const headers = gateHeaders();
  headers.set("Location", next);
  headers.append(
    "Set-Cookie",
    serializeCookie(COOKIE_NAME, AUTH_MARKER, {
      path: "/",
      maxAge: COOKIE_MAX_AGE_SECONDS,
      httpOnly: true,
      sameSite: "Lax",
      secure: shouldUseSecureCookie(url),
    }),
  );

  return new Response(null, { status: 302, headers });
}

function sanitizeNext(value: string): string {
  if (!value.startsWith("/") || value.startsWith("//")) return "/";

  for (const char of value) {
    const code = char.charCodeAt(0);
    if (char === "\\" || code < 0x20 || code === 0x7f) return "/";
  }

  return value;
}

async function timingSafeEqual(provided: string, expected: string): Promise<boolean> {
  const encoder = new TextEncoder();
  const [providedHash, expectedHash] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(provided)),
    crypto.subtle.digest("SHA-256", encoder.encode(expected)),
  ]);
  const providedBytes = new Uint8Array(providedHash);
  const expectedBytes = new Uint8Array(expectedHash);
  let diff = providedBytes.length ^ expectedBytes.length;

  for (let index = 0; index < providedBytes.length; index += 1) {
    diff |= (providedBytes[index] ?? 0) ^ (expectedBytes[index] ?? 0);
  }

  return diff === 0;
}

function shouldUseSecureCookie(url: URL): boolean {
  return !(url.protocol === "http:" && isLocalHost(url.hostname));
}

function isLocalHost(hostname: string): boolean {
  return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "[::1]";
}

function loginResponse(next: string, invalid = false): Response {
  const headers = gateHeaders("text/html; charset=utf-8");
  return new Response(renderLoginPage(next, invalid), { status: 401, headers });
}

function gateHeaders(contentType?: string): Headers {
  const headers = new Headers({
    "Cache-Control": "no-store",
    "X-Robots-Tag": "noindex",
    Vary: "Cookie",
  });

  if (contentType) headers.set("Content-Type", contentType);
  return headers;
}

function renderLoginPage(next: string, invalid: boolean): string {
  const escapedNext = escapeHtml(next);
  const error = invalid ? `<p class="error">That password did not match.</p>` : "";

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>Preview password</title>
<style>
:root{font-family:Inter,ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;color:#20242a;background:#f6f7f8;color-scheme:light}
body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px}
main{width:min(100%,390px);background:#fff;border:1px solid #d9dee3;border-radius:8px;padding:28px;box-shadow:0 18px 45px rgba(30,39,50,.08)}
p{color:#5d6470;line-height:1.55;margin:0 0 20px}
h1{font-size:1.45rem;line-height:1.2;margin:0 0 10px;color:#181b20}
label{display:block;font-size:.84rem;font-weight:650;margin-bottom:8px;color:#303741}
input[type=password]{box-sizing:border-box;width:100%;border:1px solid #b8c0ca;border-radius:6px;padding:11px 12px;font:inherit;color:#1f242b;background:#fff}
button{width:100%;margin-top:14px;border:0;border-radius:6px;padding:11px 14px;background:#176a55;color:white;font:inherit;font-weight:700;cursor:pointer}
.error{border:1px solid #e2b6b1;background:#fff3f1;color:#913b32;border-radius:6px;padding:10px 12px;margin-bottom:16px}
</style>
</head>
<body>
<main>
<h1>Preview password</h1>
<p>This static preview is shared with reviewers only.</p>
${error}
<form method="post" action="${AUTH_PATH}">
<input type="hidden" name="next" value="${escapedNext}">
<label for="password">Password</label>
<input id="password" name="password" type="password" autocomplete="current-password" autofocus>
<button type="submit">Enter preview</button>
</form>
</main>
</body>
</html>`;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
