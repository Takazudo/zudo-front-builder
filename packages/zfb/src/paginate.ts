// `zfb/paginate` — paginate an array of items into per-page route entries.
//
// Mirrors the shape of Astro's `paginate()` so a `paths()` export can emit
// one route per page. The renderer treats each returned object as
// `{ params, props }`, the same convention used elsewhere by the routing
// engine. The bundled basic-blog template ships no paginated route — it
// links this API as a next step instead — so the docs page is the worked
// example.

/** Options accepted by [`paginate`]. */
export type PaginateOptions<K extends string = string> = {
  /** Items per page. Must be a positive integer (>= 1). */
  pageSize: number;
  /** Name of the dynamic param to fill (e.g. `"page"` for `[page].tsx`). */
  param: K;
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

/**
 * Route entry returned by [`paginate`], one per page.
 *
 * The `K` generic carries the literal param name through, so consumers
 * accessing e.g. `route.params.page` get `string` rather than
 * `string | undefined` under `noUncheckedIndexedAccess`.
 */
export type PaginateRoute<T, K extends string = string> = {
  params: Record<K, string>;
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
export function paginate<T, K extends string = string>(
  items: readonly T[],
  opts: PaginateOptions<K>,
): PaginateRoute<T, K>[] {
  if (!Number.isInteger(opts.pageSize) || opts.pageSize < 1) {
    throw new RangeError(
      `paginate: pageSize must be an integer >= 1, got ${String(opts.pageSize)}`,
    );
  }
  if (typeof opts.param !== "string" || opts.param.length === 0) {
    throw new TypeError(
      `paginate: param must be a non-empty string, got ${JSON.stringify(opts.param)}`,
    );
  }
  const pageSize = opts.pageSize;
  const total = items.length;
  const lastPage = Math.max(1, Math.ceil(total / pageSize));
  const routes: PaginateRoute<T, K>[] = [];
  for (let page = 1; page <= lastPage; page++) {
    const start = (page - 1) * pageSize;
    const slice = items.slice(start, start + pageSize);
    routes.push({
      params: { [opts.param]: String(page) } as Record<K, string>,
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
