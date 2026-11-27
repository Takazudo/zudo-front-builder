export function astroTextPredicate(value, selector) {
  return {
    kind: "all",
    of: [
      { kind: "count", selector, equals: 1 },
      { kind: "text", selector, equals: value },
    ],
  };
}
export function matchesAstroText(actual, value) {
  return Array.isArray(actual) && actual.length === 2 && actual[0] === 1 && actual[1] === value;
}

/** Join the Playwright source at binding delivery to the main-document init history as of that delivery. */
export function astroProducerMatches(payload, before, expectedUrl, t0, t1, evidence) {
  const p = payload.producer;
  if (
    !p?.designatedMain ||
    p.frameUrl !== expectedUrl ||
    p.receivedNs !== String(t1) ||
    p.latestDocUUID !== payload.docUUID ||
    typeof p.latestDocNs !== "string" ||
    !/^\d+$/.test(p.latestDocNs) ||
    t1 <= t0
  )
    return false;
  const within = (r) => BigInt(r.ns) >= t0 && BigInt(r.ns) <= t1;
  const inits = evidence.docInits.filter(within),
    navs = evidence.navigations.filter(within);
  const latest = inits.at(-1);
  if (!latest) return !navs.length && payload.docUUID === before && BigInt(p.latestDocNs) <= t0;
  return (
    latest.designatedMain === true &&
    latest.url === expectedUrl &&
    latest.docUUID === payload.docUUID &&
    latest.ns === p.latestDocNs &&
    navs.length === inits.length &&
    navs.at(-1).url === expectedUrl
  );
}
