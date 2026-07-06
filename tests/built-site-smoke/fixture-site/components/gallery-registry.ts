// #1385 pt.1 repro (issue #1404): `import.meta.glob` (eager + string-literal
// form) inside a module reachable from a "use client" island. Before #1404 the
// emitted islands client bundle still contained the raw `import.meta.glob(...)`
// call and threw a TypeError at hydration; after #1404 the islands shadow
// expands it at build time, so this resolves to the two gallery modules.
const modules = import.meta.glob("./gallery/*.tsx", { eager: true }) as Record<
  string,
  { label: string }
>;

export const labels: string[] = Object.values(modules)
  .map((m) => m.label)
  .sort();
