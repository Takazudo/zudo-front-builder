// Source-map expectation: Error construction at line 6, column 9 (1-based).
const marker: string = "direct-v8-negative-before-throw";

function fail(): never {
  console.error(marker);
  throw new Error("direct-v8-authored-map-sentinel");
}

export default {
  async fetch(): Promise<Response> {
    return fail();
  },
};
