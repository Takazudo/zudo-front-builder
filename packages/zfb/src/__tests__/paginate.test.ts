import { describe, expect, it } from "vitest";

import { paginate } from "../paginate.js";

describe("paginate", () => {
  it("emits one route per page with the configured pageSize", () => {
    const items = [1, 2, 3, 4, 5];
    const routes = paginate(items, { pageSize: 2, param: "page" });
    expect(routes).toHaveLength(3);
    expect(routes[0]?.params).toEqual({ page: "1" });
    expect(routes[2]?.params).toEqual({ page: "3" });
    expect(routes[0]?.props.page.data).toEqual([1, 2]);
    expect(routes[2]?.props.page.data).toEqual([5]);
  });

  it("populates page metadata (page, lastPage, pageSize, total)", () => {
    const routes = paginate(["a", "b", "c", "d", "e"], { pageSize: 3, param: "page" });
    expect(routes).toHaveLength(2);
    const first = routes[0]?.props.page;
    expect(first).toMatchObject({ page: 1, lastPage: 2, pageSize: 3, total: 5 });
    const second = routes[1]?.props.page;
    expect(second).toMatchObject({ page: 2, lastPage: 2, pageSize: 3, total: 5 });
  });

  it("emits a single empty page when given an empty list", () => {
    const routes = paginate<number>([], { pageSize: 5, param: "page" });
    expect(routes).toHaveLength(1);
    expect(routes[0]?.props.page).toMatchObject({
      data: [],
      page: 1,
      lastPage: 1,
      pageSize: 5,
      total: 0,
    });
  });

  it("rejects non-positive pageSize", () => {
    expect(() => paginate([1], { pageSize: 0, param: "page" })).toThrow(RangeError);
    expect(() => paginate([1], { pageSize: -1, param: "page" })).toThrow(RangeError);
  });

  it("rejects fractional / non-integer pageSize", () => {
    expect(() => paginate([1], { pageSize: 1.5, param: "page" })).toThrow(RangeError);
    expect(() => paginate([1], { pageSize: 2.0001, param: "page" })).toThrow(RangeError);
    expect(() => paginate([1], { pageSize: Number.NaN, param: "page" })).toThrow(RangeError);
    expect(() => paginate([1], { pageSize: Number.POSITIVE_INFINITY, param: "page" })).toThrow(
      RangeError,
    );
  });

  it("rejects empty-string param", () => {
    expect(() => paginate([1], { pageSize: 1, param: "" })).toThrow(TypeError);
  });

  it("uses the provided param name as the params key", () => {
    const routes = paginate([1, 2, 3], { pageSize: 1, param: "p" });
    expect(routes.map((r) => r.params)).toEqual([{ p: "1" }, { p: "2" }, { p: "3" }]);
  });
});
