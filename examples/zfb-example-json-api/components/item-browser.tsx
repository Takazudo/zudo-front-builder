"use client";

import { useEffect, useState } from "preact/hooks";

import type { DemoItem } from "../lib/data";

type ItemsPayload = {
  endpoint: "items";
  q: string;
  page: number;
  per: number;
  total: number;
  pages: number;
  hasNext: boolean;
  hasPrevious: boolean;
  items: DemoItem[];
};

type SearchPayload = {
  endpoint: "search";
  q: string;
  limit: number;
  total: number;
  indexBuiltAt: string | null;
  indexBuildCount: number;
  results: Array<{
    item: DemoItem;
    score: number;
    terms: string[];
  }>;
};

const PAGE_SIZE = 6;

function endpointUrl(path: string, params: Record<string, string | number | undefined>) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && String(value).length > 0) {
      query.set(key, String(value));
    }
  }
  const suffix = query.toString();
  return suffix ? `${path}?${suffix}` : path;
}

async function readJson<T>(response: Response): Promise<T> {
  const contentType = response.headers.get("content-type") ?? "";
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText || "request failed"}`);
  }
  if (!contentType.includes("application/json")) {
    throw new Error(`Expected JSON but received ${contentType || "an empty content type"}`);
  }
  return (await response.json()) as T;
}

function formatDate(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  }).format(new Date(`${value}T00:00:00Z`));
}

function formatIndexTime(value: string | null) {
  if (!value) {
    return "not built";
  }
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(value));
}

function ItemCard({ item, score, terms }: { item: DemoItem; score?: number; terms?: string[] }) {
  return (
    <li class="item-card">
      <div class="item-card__meta">
        <span>{item.id}</span>
        <span class={`status status--${item.status}`}>{item.status}</span>
      </div>
      <h3>{item.name}</h3>
      <p>{item.summary}</p>
      <dl class="item-facts">
        <div>
          <dt>Category</dt>
          <dd>{item.category}</dd>
        </div>
        <div>
          <dt>Owner</dt>
          <dd>{item.owner}</dd>
        </div>
        <div>
          <dt>Updated</dt>
          <dd>{formatDate(item.updatedAt)}</dd>
        </div>
        {score !== undefined ? (
          <div>
            <dt>Score</dt>
            <dd>{score.toFixed(3)}</dd>
          </div>
        ) : null}
      </dl>
      <div class="tag-row" aria-label={`${item.name} tags`}>
        {item.tags.map((tag) => (
          <span key={tag}>{tag}</span>
        ))}
      </div>
      {terms && terms.length > 0 ? <p class="match-terms">Matched: {terms.join(", ")}</p> : null}
    </li>
  );
}

export default function ItemBrowser() {
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const [refreshToken, setRefreshToken] = useState(0);
  const [itemsPayload, setItemsPayload] = useState<ItemsPayload | null>(null);
  const [searchPayload, setSearchPayload] = useState<SearchPayload | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();

    async function load() {
      setLoading(true);
      setError(null);
      try {
        const [items, search] = await Promise.all([
          fetch(
            endpointUrl("/api/items", {
              q: query,
              page,
              per: PAGE_SIZE,
            }),
            { signal: controller.signal },
          ).then((response) => readJson<ItemsPayload>(response)),
          fetch(
            endpointUrl("/api/search", {
              q: query,
              limit: PAGE_SIZE,
            }),
            { signal: controller.signal },
          ).then((response) => readJson<SearchPayload>(response)),
        ]);
        setItemsPayload(items);
        setSearchPayload(search);
      } catch (err) {
        if (!controller.signal.aborted) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (!controller.signal.aborted) {
          setLoading(false);
        }
      }
    }

    void load();

    return () => controller.abort();
  }, [query, page, refreshToken]);

  return (
    <section class="api-console" aria-busy={loading}>
      <form
        class="toolbar"
        onSubmit={(event) => {
          event.preventDefault();
          setPage(1);
          setQuery(draft.trim());
        }}
      >
        <label>
          <span>Search</span>
          <input
            type="search"
            value={draft}
            placeholder="Try support, review, onboarding..."
            onInput={(event) => setDraft(event.currentTarget.value)}
          />
        </label>
        <div class="toolbar-actions">
          <button type="submit">Search</button>
          <button
            type="button"
            class="button-secondary"
            onClick={() => {
              setDraft("");
              setQuery("");
              setPage(1);
            }}
          >
            Clear
          </button>
          <button
            type="button"
            class="button-secondary"
            onClick={() => setRefreshToken((value) => value + 1)}
          >
            Refresh
          </button>
        </div>
      </form>

      {error ? <p class="error-note">API request failed: {error}</p> : null}

      <div class="metric-strip">
        <div>
          <span>Items</span>
          <strong>{itemsPayload ? itemsPayload.total : "..."}</strong>
        </div>
        <div>
          <span>Search hits</span>
          <strong>{searchPayload ? searchPayload.total : "..."}</strong>
        </div>
        <div>
          <span>Index built</span>
          <strong>{formatIndexTime(searchPayload?.indexBuiltAt ?? null)}</strong>
        </div>
        <div>
          <span>Build count</span>
          <strong>{searchPayload?.indexBuildCount ?? "..."}</strong>
        </div>
      </div>

      <div class="result-grid">
        <section class="result-panel">
          <div class="panel-header">
            <div>
              <h2>Filtered items</h2>
              <p>
                Page {itemsPayload?.page ?? page} of {itemsPayload?.pages ?? 1}
              </p>
            </div>
            <div class="pager">
              <button
                type="button"
                class="button-secondary"
                disabled={loading || !itemsPayload?.hasPrevious}
                onClick={() => setPage((value) => Math.max(1, value - 1))}
              >
                Prev
              </button>
              <button
                type="button"
                class="button-secondary"
                disabled={loading || !itemsPayload?.hasNext}
                onClick={() => setPage((value) => value + 1)}
              >
                Next
              </button>
            </div>
          </div>
          <ul class="item-list">
            {(itemsPayload?.items ?? []).map((item) => (
              <ItemCard key={item.id} item={item} />
            ))}
          </ul>
        </section>

        <section class="result-panel">
          <div class="panel-header">
            <div>
              <h2>Search ranking</h2>
              <p>{query ? `Query: ${query}` : "Showing default index sample"}</p>
            </div>
          </div>
          <ul class="item-list">
            {(searchPayload?.results ?? []).map(({ item, score, terms }) => (
              <ItemCard key={item.id} item={item} score={score} terms={terms} />
            ))}
          </ul>
        </section>
      </div>
    </section>
  );
}
