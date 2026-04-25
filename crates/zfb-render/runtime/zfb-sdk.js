// zfb SDK — module specifier "zfb" resolves to this file at build time.
//
// Public APIs:
//   paginate({ items, pageSize }) -> [
//     {
//       params: { page: "1" },
//       props: { current, total, pageSize, items: [...] }
//     },
//     ...
//   ]
//
// The runtime loader (Sub 3, zfb-render) is responsible for registering
// this module under the bare specifier "zfb" so user TSX pages can do:
//   import { paginate } from "zfb";

/**
 * Split an array of items into page slices, returning a paths()-shaped array
 * suitable for use as the return value of a `paths()` export in a [page].tsx
 * route file.
 *
 * @param {{ items: unknown[], pageSize: number }} input
 * @returns {Array<{ params: { page: string }, props: { current: number, total: number, pageSize: number, items: unknown[] } }>}
 */
export function paginate(input) {
  if (input === null || input === undefined || typeof input !== "object") {
    throw new TypeError("paginate: expected { items, pageSize }");
  }
  const { items, pageSize } = input;
  if (!Array.isArray(items)) {
    throw new TypeError("paginate: items must be an array");
  }
  if (!Number.isInteger(pageSize) || pageSize < 1) {
    throw new TypeError("paginate: pageSize must be a positive integer");
  }

  if (items.length === 0) {
    return [
      {
        params: { page: "1" },
        props: { current: 1, total: 1, pageSize, items: [] },
      },
    ];
  }

  const total = Math.ceil(items.length / pageSize);
  const out = new Array(total);
  for (let i = 0; i < total; i++) {
    out[i] = {
      params: { page: String(i + 1) },
      props: {
        current: i + 1,
        total,
        pageSize,
        items: items.slice(i * pageSize, (i + 1) * pageSize),
      },
    };
  }
  return out;
}

// Reserved for future SDK helpers / introspection.
export const __zfbVersion = "0.1.0-pre";
