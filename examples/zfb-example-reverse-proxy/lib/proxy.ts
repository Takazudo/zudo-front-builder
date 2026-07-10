const DEFAULT_PROXY_PREFIX = "/proxy/";

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

const RECONSTRUCTED_REQUEST_HEADERS = new Set(["content-length", "host"]);

const STRIPPED_UPSTREAM_RESPONSE_HEADERS = new Set([
  "content-security-policy",
  "content-security-policy-report-only",
  "set-cookie",
  "strict-transport-security",
]);

type CloudflareFetchInit = RequestInit & {
  cf?: {
    cacheEverything?: boolean;
  };
};

export type ProxyRequestOptions = {
  request: Request;
  origin?: string;
  proxyPrefix?: string;
};

const CLOUDFLARE_FETCH_HINTS: CloudflareFetchInit = {
  cf: {
    cacheEverything: true,
  },
};

export async function proxyRequest({
  request,
  origin,
  proxyPrefix = DEFAULT_PROXY_PREFIX,
}: ProxyRequestOptions): Promise<Response> {
  if (!origin) {
    return proxyError(500, "PROXY_ORIGIN is not configured.");
  }

  let upstreamUrl: URL;
  try {
    upstreamUrl = buildUpstreamUrl(request.url, origin, proxyPrefix);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return proxyError(500, message);
  }

  const upstreamRequest = new Request(upstreamUrl, {
    method: request.method,
    headers: filteredRequestHeaders(request.headers),
    body: requestAllowsBody(request) ? request.body : undefined,
    redirect: "manual",
  });

  let upstreamResponse: Response;
  try {
    upstreamResponse = await fetch(upstreamRequest, CLOUDFLARE_FETCH_HINTS);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return proxyError(502, `Upstream fetch failed: ${message}`);
  }

  return proxyResponse(upstreamResponse, {
    upstreamRequestUrl: upstreamUrl,
    proxyPrefix,
  });
}

export function buildUpstreamUrl(
  requestUrl: string | URL,
  origin: string,
  proxyPrefix = DEFAULT_PROXY_PREFIX,
): URL {
  const incomingUrl = new URL(requestUrl);
  const upstreamOrigin = normalizeProxyOrigin(origin);
  const prefix = normalizeProxyPrefix(proxyPrefix);

  if (!incomingUrl.pathname.startsWith(prefix)) {
    throw new Error(`Request path must start with ${prefix}.`);
  }

  const pathRemainder = incomingUrl.pathname.slice(prefix.length);
  upstreamOrigin.pathname = joinPaths(upstreamOrigin.pathname, pathRemainder);
  upstreamOrigin.search = incomingUrl.search;
  upstreamOrigin.hash = "";
  return upstreamOrigin;
}

export function filteredRequestHeaders(source: Headers): Headers {
  const headers = new Headers();
  source.forEach((value, name) => {
    if (shouldStripRequestHeader(name)) return;
    headers.append(name, value);
  });
  return headers;
}

export function filteredResponseHeaders(
  source: Headers,
  options: {
    upstreamRequestUrl: URL;
    proxyPrefix?: string;
  },
): Headers {
  const headers = new Headers();

  source.forEach((value, name) => {
    if (shouldStripResponseHeader(name)) return;

    if (name.toLowerCase() === "location") {
      headers.set(
        "location",
        rewriteLocationHeader(value, {
          upstreamRequestUrl: options.upstreamRequestUrl,
          proxyPrefix: options.proxyPrefix ?? DEFAULT_PROXY_PREFIX,
        }),
      );
      return;
    }

    headers.append(name, value);
  });

  return headers;
}

export function proxyResponse(
  upstreamResponse: Response,
  options: {
    upstreamRequestUrl: URL;
    proxyPrefix?: string;
  },
): Response {
  return new Response(upstreamResponse.body, {
    status: upstreamResponse.status,
    statusText: upstreamResponse.statusText,
    headers: filteredResponseHeaders(upstreamResponse.headers, options),
  });
}

export function rewriteLocationHeader(
  location: string,
  options: {
    upstreamRequestUrl: URL;
    proxyPrefix?: string;
  },
): string {
  let target: URL;
  try {
    target = new URL(location, options.upstreamRequestUrl);
  } catch {
    return location;
  }

  if (target.origin !== options.upstreamRequestUrl.origin) {
    return location;
  }

  const prefix = normalizeProxyPrefix(options.proxyPrefix ?? DEFAULT_PROXY_PREFIX);
  const pathRemainder = target.pathname.replace(/^\/+/, "");
  return `${prefix}${pathRemainder}${target.search}${target.hash}`;
}

export function shouldStripRequestHeader(name: string): boolean {
  const normalized = name.toLowerCase();
  return (
    isHopByHopHeader(normalized) ||
    normalized.startsWith("proxy-") ||
    RECONSTRUCTED_REQUEST_HEADERS.has(normalized)
  );
}

export function shouldStripResponseHeader(name: string): boolean {
  const normalized = name.toLowerCase();
  return (
    isHopByHopHeader(normalized) ||
    normalized.startsWith("proxy-") ||
    STRIPPED_UPSTREAM_RESPONSE_HEADERS.has(normalized)
  );
}

function isHopByHopHeader(normalizedName: string): boolean {
  return HOP_BY_HOP_HEADERS.has(normalizedName);
}

function requestAllowsBody(request: Request): boolean {
  return request.method !== "GET" && request.method !== "HEAD";
}

function normalizeProxyOrigin(origin: string): URL {
  const url = new URL(origin.trim());
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("PROXY_ORIGIN must be an absolute http or https URL.");
  }
  url.username = "";
  url.password = "";
  url.search = "";
  url.hash = "";
  return url;
}

function normalizeProxyPrefix(prefix: string): string {
  if (!prefix.startsWith("/")) {
    throw new Error("Proxy prefix must start with /.");
  }
  return prefix.endsWith("/") ? prefix : `${prefix}/`;
}

function joinPaths(basePath: string, pathRemainder: string): string {
  const base = basePath === "/" ? "" : basePath.replace(/\/+$/, "");
  const remainder = pathRemainder.replace(/^\/+/, "");
  if (!remainder) return base || "/";
  return `${base}/${remainder}`;
}

function proxyError(status: number, message: string): Response {
  return new Response(`${message}\n`, {
    status,
    headers: {
      "cache-control": "no-store",
      "content-type": "text/plain; charset=utf-8",
    },
  });
}
