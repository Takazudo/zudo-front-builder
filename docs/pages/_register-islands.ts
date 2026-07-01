// Island registration for zudo-doc v2 package-owned routes.
//
// The package's injected doc routes render the doc-history area via
// `createChrome()` with NO hostBindings, so `deriveDocHistorySlot` resolves to
// the no-op stub and the REAL interactive DocHistory client island
// (@takazudo/zudo-doc/doc-history, a "use client" module) is never pulled into
// zfb's island scan graph — its "DocHistory" SSR marker then has no client
// binding and cannot hydrate (build warning: "island marker DocHistory … has
// no matching registry entry"). Re-exporting it from this pages/-reachable
// module registers the "DocHistory" client binding; zfb matches the marker by
// name and the serialized {slug, locale, basePath} props line up with the real
// component's signature. (Reported upstream: package-owned routes need a seam
// to register/inject the DocHistory island under docHistory:true.)
export { DocHistory } from "@takazudo/zudo-doc/doc-history";
