import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";

import { filterItems } from "../../lib/data";
import { jsonResponse, methodNotAllowed, preflightResponse } from "../../lib/cors";

export const prerender = false;

interface Env {}

function readBoundedInteger(value: string | null, fallback: number, min: number, max: number) {
  const parsed = Number.parseInt(value ?? "", 10);
  if (!Number.isFinite(parsed)) {
    return fallback;
  }
  return Math.min(max, Math.max(min, parsed));
}

export default async function ItemsApi() {
  const { request } = getCloudflareContext<Env>();
  const preflight = preflightResponse(request);
  if (preflight) {
    return preflight;
  }
  if (request.method !== "GET") {
    return methodNotAllowed(request.method);
  }

  const url = new URL(request.url);
  const q = url.searchParams.get("q") ?? "";
  const requestedPage = readBoundedInteger(url.searchParams.get("page"), 1, 1, 1000);
  const per = readBoundedInteger(url.searchParams.get("per"), 8, 1, 50);
  const matches = filterItems(q);
  const total = matches.length;
  const pages = Math.max(1, Math.ceil(total / per));
  const page = Math.min(requestedPage, pages);
  const start = (page - 1) * per;
  const items = matches.slice(start, start + per);

  return jsonResponse({
    endpoint: "items",
    q: q.trim(),
    page,
    per,
    total,
    pages,
    hasNext: page < pages,
    hasPrevious: page > 1,
    items,
  });
}
