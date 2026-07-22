// PROTOTYPE benchmark core (zfb#1855, epic zfb#1854). Environment-agnostic
// measurement helpers shared by the Node runner and the browser harness —
// no Node imports here; `performance.now()` exists in both environments.

/** Steady-state stats over per-iteration millisecond samples. */
export function stats(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const mean = sorted.reduce((acc, x) => acc + x, 0) / sorted.length;
  const p95 = sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.95) - 1)];
  return { n: sorted.length, meanMs: mean, p95Ms: p95 };
}

const isThenable = (value) => value != null && typeof value.then === "function";

/**
 * Warm up, then measure `fn` (sync or async) once per iteration.
 * Per-iteration timing (not batched) so p95 is a real tail statistic.
 *
 * Fairness (self-review finding P2): the timer only awaits when `fn`
 * actually returns a thenable. An unconditional `await fn()` would suspend
 * through a promise continuation even for remark's synchronous `parse()`,
 * charging async scheduling overhead to the sync side; the wasm side's
 * await stays — its async wrapper is the real shipped consumer cost.
 */
export async function measure(fn, { iterations, warmup }) {
  for (let i = 0; i < warmup; i += 1) {
    const r = fn();
    if (isThenable(r)) await r;
  }
  const samples = new Array(iterations);
  for (let i = 0; i < iterations; i += 1) {
    const t0 = performance.now();
    const r = fn();
    if (isThenable(r)) await r;
    samples[i] = performance.now() - t0;
  }
  return stats(samples);
}

/**
 * Run the full corpus × implementations grid sequentially (never
 * interleaved within a doc, so JIT warmup states don't cross-pollute
 * mid-measurement).
 *
 * `impls`: `[{ name, fn(doc) }]`. Returns
 * `[{ doc, bytes, results: { [implName]: { n, meanMs, p95Ms } } }]`.
 */
export async function runGrid(corpus, impls, { onProgress } = {}) {
  const rows = [];
  for (const { name, doc, iterations, warmup } of corpus) {
    const results = {};
    for (const impl of impls) {
      onProgress?.(`${name} × ${impl.name} (${iterations} iters)`);
      results[impl.name] = await measure(() => impl.fn(doc), { iterations, warmup });
    }
    rows.push({ doc: name, bytes: byteLength(doc), results });
  }
  return rows;
}

export function byteLength(text) {
  return new TextEncoder().encode(text).length;
}

const fmt = (ms) => (ms >= 100 ? ms.toFixed(1) : ms >= 10 ? ms.toFixed(2) : ms.toFixed(3));

/** Render the grid as a GitHub-flavored markdown table. */
export function markdownTable(rows, implNames) {
  const header = [
    "doc",
    "bytes",
    ...implNames.flatMap((n) => [`${n} mean (ms)`, `${n} p95 (ms)`]),
    "ratio (mean)",
  ];
  const lines = [`| ${header.join(" | ")} |`, `| ${header.map(() => "---").join(" | ")} |`];
  for (const row of rows) {
    const cells = [row.doc, String(row.bytes)];
    for (const name of implNames) {
      const r = row.results[name];
      cells.push(fmt(r.meanMs), fmt(r.p95Ms));
    }
    const [a, b] = implNames;
    const ratio = row.results[b].meanMs / row.results[a].meanMs;
    cells.push(`${ratio.toFixed(2)}x`);
    lines.push(`| ${cells.join(" | ")} |`);
  }
  return lines.join("\n");
}
