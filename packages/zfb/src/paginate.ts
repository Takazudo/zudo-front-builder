// `zfb/paginate` — paginate an array of items into per-page route entries.
//
// Mirrors the shape of Astro's `paginate()` so the basic-blog example can
// emit one route per page from a `paths()` export. The renderer treats
// each returned object as `{ params, props }`, the same convention used
// elsewhere by the routing engine.

/** Options accepted by [`paginate`]. */
export type PaginateOptions = {
  /** Items per page. Must be >= 1. */
  pageSize: number;
  /** Name of the dynamic param to fill (e.g. `"page"` for `[page].tsx`). */
  param: string;
};

/** One page of paginated results. The shape consumed by the page component. */
export type PaginatedPage<T> = {
  /** Items belonging to this page. */
  data: T[];
  /** 1-based page number. */
  page: number;
  /** Total number of pages (>= 1). */
  lastPage: number;
  /** Items per page (echoed for convenience). */
  pageSize: number;
  /** Total item count across all pages. */
  total: number;
};

/** Route entry returned by [`paginate`], one per page. */
export type PaginateRoute<T> = {
  params: Record<string, string>;
  props: { page: PaginatedPage<T> };
};

/**
 * Slice `items` into pages of `opts.pageSize` and produce one route entry
 * per page. The `param` option names the dynamic segment to fill — for
 * `pages/blog/page/[page].tsx` pass `param: "page"`.
 *
 * Always emits at least one route. An empty input list yields a single
 * empty page so the index route still renders rather than 404ing.
 */
export function paginate<T>(items: readonly T[], opts: PaginateOptions): PaginateRoute<T>[] {
  if (!Number.isFinite(opts.pageSize) || opts.pageSize < 1) {
    throw new RangeError(`paginate: pageSize must be >= 1, got ${opts.pageSize}`);
  }
  const pageSize = Math.floor(opts.pageSize);
  const total = items.length;
  const lastPage = Math.max(1, Math.ceil(total / pageSize));
  const routes: PaginateRoute<T>[] = [];
  for (let page = 1; page <= lastPage; page++) {
    const start = (page - 1) * pageSize;
    const slice = items.slice(start, start + pageSize);
    routes.push({
      params: { [opts.param]: String(page) },
      props: {
        page: {
          data: slice,
          page,
          lastPage,
          pageSize,
          total,
        },
      },
    });
  }
  return routes;
}
