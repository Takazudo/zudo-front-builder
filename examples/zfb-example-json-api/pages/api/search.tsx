import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";
import MiniSearch from "minisearch";

import { ITEM_BY_ID, ITEMS, normalizeQuery, type DemoItem } from "../../lib/data";
import { jsonResponse, methodNotAllowed, preflightResponse } from "../../lib/cors";

export const prerender = false;

interface Env {}

type SearchDocument = DemoItem & {
  tagsText: string;
};

let searchIndex: MiniSearch<SearchDocument> | null = null;
let indexBuiltAt: string | null = null;
let indexBuildCount = 0;

function getSearchIndex() {
  if (!searchIndex) {
    searchIndex = new MiniSearch<SearchDocument>({
      fields: ["name", "category", "owner", "summary", "tagsText"],
      storeFields: ["id"],
      searchOptions: {
        boost: {
          name: 3,
          tagsText: 2,
          category: 1.5,
        },
        fuzzy: 0.2,
        prefix: true,
      },
    });
    searchIndex.addAll(
      ITEMS.map((item) => ({
        ...item,
        tagsText: item.tags.join(" "),
      })),
    );
    indexBuiltAt = new Date().toISOString();
    indexBuildCount += 1;
  }

  return {
    index: searchIndex,
    builtAt: indexBuiltAt,
    buildCount: indexBuildCount,
  };
}

function readLimit(value: string | null) {
  const parsed = Number.parseInt(value ?? "", 10);
  if (!Number.isFinite(parsed)) {
    return 8;
  }
  return Math.min(20, Math.max(1, parsed));
}

export default async function SearchApi() {
  const { request } = getCloudflareContext<Env>();
  const preflight = preflightResponse(request);
  if (preflight) {
    return preflight;
  }
  if (request.method !== "GET") {
    return methodNotAllowed(request.method);
  }

  const url = new URL(request.url);
  const q = normalizeQuery(url.searchParams.get("q"));
  const limit = readLimit(url.searchParams.get("limit"));
  const { index, builtAt, buildCount } = getSearchIndex();

  const results = q
    ? index
        .search(q)
        .slice(0, limit)
        .flatMap((result) => {
          const item = ITEM_BY_ID.get(String(result.id));
          if (!item) {
            return [];
          }
          return [
            {
              item,
              score: Number(result.score.toFixed(3)),
              terms: result.terms,
            },
          ];
        })
    : ITEMS.slice(0, limit).map((item, index) => ({
        item,
        score: Number((1 / (index + 1)).toFixed(3)),
        terms: [],
      }));

  return jsonResponse({
    endpoint: "search",
    q,
    limit,
    total: results.length,
    indexBuiltAt: builtAt,
    indexBuildCount: buildCount,
    results,
  });
}
