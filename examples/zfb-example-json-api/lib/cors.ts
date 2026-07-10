const CORS_HEADERS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "GET, OPTIONS",
  "Access-Control-Allow-Headers": "Content-Type",
  "Access-Control-Max-Age": "86400",
};

export function corsHeaders(init?: HeadersInit): Headers {
  const headers = new Headers(init);
  for (const [name, value] of Object.entries(CORS_HEADERS)) {
    headers.set(name, value);
  }
  return headers;
}

export function preflightResponse(request: Request): Response | null {
  if (request.method !== "OPTIONS") {
    return null;
  }
  return new Response(null, {
    status: 204,
    headers: corsHeaders({
      Allow: "GET, OPTIONS",
    }),
  });
}

export function jsonResponse(data: unknown, init: ResponseInit = {}): Response {
  const headers = corsHeaders(init.headers);
  if (!headers.has("content-type")) {
    headers.set("content-type", "application/json; charset=utf-8");
  }
  if (!headers.has("cache-control")) {
    headers.set("cache-control", "no-store");
  }
  return new Response(JSON.stringify(data, null, 2), {
    ...init,
    headers,
  });
}

export function methodNotAllowed(method: string): Response {
  return jsonResponse(
    {
      error: "method_not_allowed",
      message: `Method ${method} is not allowed. Use GET.`,
    },
    {
      status: 405,
      headers: {
        Allow: "GET, OPTIONS",
      },
    },
  );
}
