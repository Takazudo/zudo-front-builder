// Unit tests for the zfb SDK paginate() helper.
//
// Run with Node 20+'s built-in test runner:
//   node --test crates/zfb-render/runtime/__tests__/zfb-sdk.test.mjs
//
// These tests are deliberately self-contained and have no third-party
// dependencies so they can run before Sub 3's zfb-render Cargo crate is
// even scaffolded.

import { test } from "node:test";
import assert from "node:assert/strict";

import { paginate, __zfbVersion } from "../zfb-sdk.js";

test("paginate: even split (10 items, pageSize 3 -> 4 pages: 3+3+3+1)", () => {
  const items = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
  const result = paginate({ items, pageSize: 3 });

  assert.equal(result.length, 4);

  assert.deepEqual(result[0], {
    params: { page: "1" },
    props: { current: 1, total: 4, pageSize: 3, items: [1, 2, 3] },
  });
  assert.deepEqual(result[1], {
    params: { page: "2" },
    props: { current: 2, total: 4, pageSize: 3, items: [4, 5, 6] },
  });
  assert.deepEqual(result[2], {
    params: { page: "3" },
    props: { current: 3, total: 4, pageSize: 3, items: [7, 8, 9] },
  });
  assert.deepEqual(result[3], {
    params: { page: "4" },
    props: { current: 4, total: 4, pageSize: 3, items: [10] },
  });
});

test("paginate: exact split (9 items, pageSize 3 -> 3 pages of 3)", () => {
  const items = [1, 2, 3, 4, 5, 6, 7, 8, 9];
  const result = paginate({ items, pageSize: 3 });

  assert.equal(result.length, 3);
  for (const page of result) {
    assert.equal(page.props.pageSize, 3);
    assert.equal(page.props.total, 3);
    assert.equal(page.props.items.length, 3);
  }
  assert.deepEqual(result[2].props.items, [7, 8, 9]);
});

test("paginate: empty array -> 1 page with []", () => {
  const result = paginate({ items: [], pageSize: 5 });

  assert.equal(result.length, 1);
  assert.deepEqual(result[0], {
    params: { page: "1" },
    props: { current: 1, total: 1, pageSize: 5, items: [] },
  });
});

test("paginate: single item", () => {
  const result = paginate({ items: ["only"], pageSize: 10 });

  assert.equal(result.length, 1);
  assert.deepEqual(result[0], {
    params: { page: "1" },
    props: { current: 1, total: 1, pageSize: 10, items: ["only"] },
  });
});

test("paginate: pageSize > items.length", () => {
  const result = paginate({ items: [1, 2], pageSize: 50 });

  assert.equal(result.length, 1);
  assert.deepEqual(result[0], {
    params: { page: "1" },
    props: { current: 1, total: 1, pageSize: 50, items: [1, 2] },
  });
});

test("paginate: pageSize 1 produces one page per item", () => {
  const result = paginate({ items: ["a", "b", "c"], pageSize: 1 });

  assert.equal(result.length, 3);
  assert.deepEqual(
    result.map((p) => p.params.page),
    ["1", "2", "3"],
  );
  assert.deepEqual(
    result.map((p) => p.props.items),
    [["a"], ["b"], ["c"]],
  );
  for (const page of result) {
    assert.equal(page.props.total, 3);
    assert.equal(page.props.pageSize, 1);
  }
});

test("paginate: page numbers are stringified", () => {
  const items = Array.from({ length: 12 }, (_, i) => i);
  const result = paginate({ items, pageSize: 5 });

  assert.equal(result.length, 3);
  for (const page of result) {
    assert.equal(typeof page.params.page, "string");
  }
  assert.deepEqual(
    result.map((p) => p.params.page),
    ["1", "2", "3"],
  );
});

test("paginate: does not mutate the input items array", () => {
  const items = [1, 2, 3, 4, 5];
  const snapshot = items.slice();
  paginate({ items, pageSize: 2 });
  assert.deepEqual(items, snapshot);
});

test("paginate: works with arbitrary object items", () => {
  const items = [
    { slug: "a", title: "Alpha" },
    { slug: "b", title: "Beta" },
    { slug: "c", title: "Gamma" },
  ];
  const result = paginate({ items, pageSize: 2 });

  assert.equal(result.length, 2);
  assert.deepEqual(result[0].props.items, [
    { slug: "a", title: "Alpha" },
    { slug: "b", title: "Beta" },
  ]);
  assert.deepEqual(result[1].props.items, [{ slug: "c", title: "Gamma" }]);
  // Same reference identity for items (slice returns the same object refs).
  assert.equal(result[0].props.items[0], items[0]);
});

test("paginate: rejects non-object input", () => {
  assert.throws(() => paginate(null), TypeError);
  assert.throws(() => paginate(undefined), TypeError);
  assert.throws(() => paginate(42), TypeError);
  assert.throws(() => paginate("nope"), TypeError);
});

test("paginate: rejects non-array items", () => {
  assert.throws(() => paginate({ items: "abc", pageSize: 2 }), TypeError);
  assert.throws(() => paginate({ items: null, pageSize: 2 }), TypeError);
  assert.throws(() => paginate({ items: { 0: "a" }, pageSize: 2 }), TypeError);
});

test("paginate: rejects invalid pageSize", () => {
  assert.throws(() => paginate({ items: [], pageSize: 0 }), TypeError);
  assert.throws(() => paginate({ items: [], pageSize: -1 }), TypeError);
  assert.throws(() => paginate({ items: [], pageSize: 1.5 }), TypeError);
  assert.throws(() => paginate({ items: [], pageSize: "3" }), TypeError);
  assert.throws(() => paginate({ items: [], pageSize: NaN }), TypeError);
});

test("__zfbVersion is exported as a string", () => {
  assert.equal(typeof __zfbVersion, "string");
  assert.ok(__zfbVersion.length > 0);
});
