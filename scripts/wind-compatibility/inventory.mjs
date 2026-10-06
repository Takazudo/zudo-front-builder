import { createHash } from "node:crypto";
import { readFile, lstat, realpath } from "node:fs/promises";
import { gunzipSync } from "node:zlib";
import { pathToFileURL } from "node:url";
import { basename, dirname, resolve } from "node:path";

export const PIN = Object.freeze({
  package: "tailwindcss",
  version: "4.3.2",
  tag: "v4.3.2",
  observedTagCommit: "056a1550721d4bf79ff732d5ab9414fa83f7064f",
  tagToArtifactLink: "unverified",
  archiveSha256: "1d73680e19488b19e97ea3c96722e363cc0c4fd118af546857d8703e8f0f9be3",
  sourcePrefix: "tailwindcss-056a1550721d4bf79ff732d5ab9414fa83f7064f/packages/tailwindcss/src/",
});
export const WIND_SOURCE_SHA = "6ad10cfcb1eeeca0288d577b88f8a9312828cc83";
export const digest = (data) => createHash("sha256").update(data).digest("hex");

export function archiveSources(bytes) {
  if (digest(bytes) !== PIN.archiveSha256) throw Error("Pinned source archive digest mismatch");
  const tar = gunzipSync(bytes),
    files = {};
  for (let at = 0; at + 512 <= tar.length;) {
    const entryName = tar.toString("utf8", at, at + 100).replace(/\0.*$/, "");
    if (!entryName) break;
    const prefix = tar.toString("utf8", at + 345, at + 500).replace(/\0.*$/, "");
    const name = prefix ? `${prefix}/${entryName}` : entryName;
    const size = parseInt(
      tar
        .toString("ascii", at + 124, at + 136)
        .replace(/\0.*$/, "")
        .trim(),
      8,
    );
    if (!Number.isFinite(size)) throw Error("Invalid source archive");
    for (const file of [
      "utilities.ts",
      "variants.ts",
      "utilities.test.ts",
      "variants.test.ts",
      "compat/legacy-utilities.ts",
      "compat/legacy-utilities.test.ts",
    ]) {
      if (name === PIN.sourcePrefix + file)
        files[file] = tar.toString("utf8", at + 512, at + 512 + size);
    }
    at += 512 + Math.ceil(size / 512) * 512;
  }
  if (Object.keys(files).length !== 6) throw Error("Pinned source files missing");
  return files;
}

function finiteLoopRegistrations(source) {
  const found = { static: [], functional: [], sites: [] };
  const loop = /^(\s*)for \(let \[([^\]]+)\] of \[/gm;
  for (const match of source.matchAll(loop)) {
    const indent = match[1].split("\n").at(-1).length;
    const variable = match[2].split(",")[0].trim();
    const begin = match.index + match[0].length;
    // Find the array terminator by balanced brackets; tuple/theme arrays may be nested.
    let depth = 1,
      at = begin,
      quote = null;
    for (; at < source.length && depth; at++) {
      const ch = source[at];
      if (quote) {
        if (ch === quote && source[at - 1] !== "\\") quote = null;
        continue;
      }
      if (ch === "'" || ch === '"' || ch === "`") {
        quote = ch;
        continue;
      }
      if (ch === "[") depth++;
      else if (ch === "]") depth--;
    }
    if (depth) continue;
    const arrayText = source.slice(begin, at - 1);
    const openBrace = source.indexOf("{", at);
    if (openBrace < 0 || openBrace - at > 100) continue;
    const closing = new RegExp(`^ {${indent}}\\}`, "m");
    const tail = source.slice(openBrace + 1);
    const closeMatch = closing.exec(tail);
    if (!closeMatch) continue;
    const body = tail.slice(0, closeMatch.index);
    const values = [...arrayText.matchAll(/\[\s*'([^']+)'\s*,/g)].map((m) => m[1]);
    const template = new RegExp(
      "\\b(?:staticUtility|utilities\\.static)\\(\\s*`([^`]*\\$\\{" + variable + "\\}[^`]*)`",
      "g",
    );
    const functional = new RegExp(
      "\\b(?:functionalUtility|spacingUtility)\\(\\s*" + variable + "\\b",
      "g",
    );
    const templates = [...body.matchAll(template)].map((m) => m[1]);
    const hasFunctional = functional.test(body);
    if (!values.length || (!templates.length && !hasFunctional)) continue;
    for (const value of values) {
      for (const t of templates) found.static.push(t.replaceAll("${" + variable + "}", value));
      if (hasFunctional) found.functional.push(value);
    }
    found.sites.push({
      line: source.slice(0, match.index).split("\n").length,
      endLine: source.slice(0, openBrace + closeMatch.index + 1).split("\n").length,
      variable,
      values,
      templates,
      functional: hasFunctional,
    });
  }
  const scalarLoop = /^(\s*)for \(let (\w+) of \[([^\]]+)\]\) \{/gm;
  for (const match of source.matchAll(scalarLoop)) {
    const indent = match[1].split("\n").at(-1).length;
    const variable = match[2];
    const values = [...match[3].matchAll(/'([^']+)'/g)].map((m) => m[1]);
    const tail = source.slice(match.index + match[0].length);
    const closeMatch = new RegExp(`^ {${indent}}\\}`, "m").exec(tail);
    if (!values.length || !closeMatch) continue;
    const body = tail.slice(0, closeMatch.index);
    const template = new RegExp(
      "\\b(?:staticUtility|utilities\\.static)\\(\\s*`([^`]*\\$\\{" + variable + "\\}[^`]*)`",
      "g",
    );
    const functional = new RegExp(
      "\\b(?:functionalUtility|spacingUtility)\\(\\s*`([^`]*\\$\\{" + variable + "\\}[^`]*)`",
      "g",
    );
    const templates = [...body.matchAll(template)].map((m) => m[1]);
    const patterns = [...body.matchAll(functional)].map((m) => m[1]);
    if (!templates.length && !patterns.length) continue;
    for (const value of values) {
      for (const t of templates) found.static.push(t.replaceAll("${" + variable + "}", value));
      for (const t of patterns) found.functional.push(t.replaceAll("${" + variable + "}", value));
    }
    found.sites.push({
      line: source.slice(0, match.index).split("\n").length,
      endLine: source.slice(0, match.index + match[0].length + closeMatch.index).split("\n").length,
      variable,
      values,
      templates,
      functionalTemplates: patterns,
    });
  }
  return found;
}

export function extractSurface(files) {
  const utilities = files["utilities.ts"] + "\n" + files["compat/legacy-utilities.ts"],
    variants = files["variants.ts"];
  if (
    !utilities ||
    !variants ||
    !files["utilities.test.ts"] ||
    !files["variants.test.ts"] ||
    !files["compat/legacy-utilities.test.ts"]
  )
    throw Error("Source/test set incomplete");
  const collect = (source, pattern) =>
    [...source.matchAll(pattern)].map((match) => match[1]).sort();
  const unique = (values) => [...new Set(values)].sort();
  const finite = finiteLoopRegistrations(utilities);
  const staticUtilities = unique([
    ...finite.static,
    ...collect(utilities, /\b(?:staticUtility|utilities\.static)\(\s*'([^']+)'/g),
    ...collect(utilities, /\b(?:staticUtility|utilities\.static)\(\s*`([^`$]+)`/g),
  ]);
  const functionalUtilities = unique([
    ...finite.functional,
    ...collect(
      utilities,
      /\b(?:functionalUtility|utilities\.functional|spacingUtility)\(\s*'([^']+)'/g,
    ),
    ...collect(utilities, /\b(?:functionalUtility|utilities\.functional)\(\s*`([^`$]+)`/g),
  ]);
  const staticVariants = unique(
    collect(variants, /\b(?:staticVariant|variants\.static)\(\s*'([^']+)'/g),
  );
  const functionalVariants = unique(collect(variants, /\bvariants\.functional\(\s*'([^']+)'/g));
  const compoundVariants = unique(collect(variants, /\bvariants\.compound\(\s*'([^']+)'/g));
  const probeNames = [
    "mx-auto",
    "p-0",
    "table",
    "appearance-none",
    "order-first",
    "basis-auto",
    "fill-none",
    "sr-only",
    "hover:block",
  ];
  const testExamples = Object.fromEntries(
    probeNames.map((name) => [
      name,
      Object.fromEntries(
        ["utilities.test.ts", "variants.test.ts", "compat/legacy-utilities.test.ts"].map((file) => {
          const lines = files[file].split("\n");
          const hits = lines.flatMap((line, index) => (line.includes(name) ? [index + 1] : []));
          return [file, hits.slice(0, 8)];
        }),
      ),
    ]),
  );
  const functionalConstraints = Object.fromEntries(
    functionalUtilities.map((name) => {
      const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const match = new RegExp(`\\b(?:functionalUtility|spacingUtility)\\(\\s*'${escaped}'`).exec(
        utilities,
      );
      const closing = match
        ? utilities.indexOf(match[0].startsWith("spacingUtility") ? "\n  )" : "\n  })", match.index)
        : -1;
      const excerpt = match
        ? utilities.slice(
            match.index,
            closing > match.index && closing - match.index < 4000 ? closing + 5 : match.index + 500,
          )
        : "";
      const themeKeys = [...excerpt.matchAll(/'(--[a-z0-9-]+)'/g)]
        .map((m) => m[1])
        .filter((v) => !v.startsWith("--tw-"));
      return [
        name,
        {
          domain: excerpt
            ? "source-inspected-functional-registration"
            : "finite-loop-helper-registration-requires-review",
          themeKeys: [...new Set(themeKeys)],
          bareValueHandler: /handleBareValue/.test(excerpt),
          negative: /supportsNegative: true/.test(excerpt),
          fractions: /supportsFractions: true/.test(excerpt),
          staticValueKeys:
            excerpt
              .match(/staticValues:\s*\{([^}]+)\}/)?.[1]
              ?.match(/\b[a-z][a-z0-9-]*:/g)
              ?.map((x) => x.slice(0, -1)) ?? [],
        },
      ];
    }),
  );
  const dynamicSourceSites = [];
  for (const [file, source, pattern] of [
    [
      "utilities.ts",
      utilities,
      /\b(?:staticUtility|functionalUtility|spacingUtility|utilities\.static|utilities\.functional)\(\s*([^,\n]{1,90})/g,
    ],
    [
      "variants.ts",
      variants,
      /\b(?:staticVariant|variants\.static|variants\.functional|variants\.compound)\(\s*([^,\n]{1,90})/g,
    ],
  ]) {
    const occurrences = new Map();
    for (const match of source.matchAll(pattern)) {
      const expression = match[1].trim();
      if (expression.startsWith("'") || (expression.startsWith("`") && !expression.includes("${")))
        continue;
      const key = `${file}:${digest(match[0]).slice(0, 12)}`;
      const ordinal = (occurrences.get(key) ?? 0) + 1;
      occurrences.set(key, ordinal);
      dynamicSourceSites.push({
        id: `${key}:${ordinal}`,
        file,
        line: source.slice(0, match.index).split("\n").length,
        expression,
      });
    }
  }
  const sourceDigests = Object.fromEntries(
    Object.entries(files).map(([name, text]) => [name, digest(text)]),
  );
  return {
    schemaVersion: 1,
    pin: PIN,
    sourceDigests,
    extraction:
      "literal registrations in utilities.ts and variants.ts; dynamic/template registrations and token domains are not enumerated",
    staticUtilities,
    functionalUtilities,
    staticVariants,
    functionalVariants,
    compoundVariants,
    finiteLoopSites: finite.sites,
    functionalConstraints,
    dynamicSourceSites,
    testExamples,
    dynamicRegistrationSites: {
      utilities: (
        utilities.match(
          /\b(?:staticUtility|functionalUtility|utilities\.static|utilities\.functional)\(\s*(?!')/g,
        ) ?? []
      ).length,
      variants: (
        variants.match(/\b(?:staticVariant|variants\.static|variants\.functional)\(\s*(?!')/g) ?? []
      ).length,
    },
    testsInspected: ["utilities.test.ts", "variants.test.ts", "compat/legacy-utilities.test.ts"],
  };
}

const issue = (n) => `https://github.com/Takazudo/zudo-front-builder/issues/${n}`;
const nativeCandidate = (name) =>
  /^(?:table(?:-|$)|inline-table$|contents$|flow-root$|list-item$|run-in$|appearance-|order-|basis-|fill-|stroke-|sr-only$|not-sr-only$|forced-color-adjust-|pointer-events-|select-)/.test(
    name,
  );
const tracking = (name) =>
  /^(?:ring|animate)/.test(name)
    ? issue(3372)
    : /^(?:space-|divide-)/.test(name)
      ? issue(3386)
      : null;
function windExact(name, catalog) {
  return catalog.entries.find((entry) =>
    entry.acceptedValues?.some((value) => {
      if (!["exact", "keyword"].includes(value.kind)) return false;
      const positive = value.suffix === null ? entry.root : `${entry.root}-${value.suffix}`;
      if (name === positive) return true;
      return (
        entry.negativePolicy === "allowed" && name === `-${positive}` && value.suffix !== "auto"
      );
    }),
  );
}

const axisSemantics = {
  mx: ["margin-inline", "margin-left/right"],
  my: ["margin-block", "margin-top/bottom"],
  px: ["padding-inline", "padding-left/right"],
  py: ["padding-block", "padding-top/bottom"],
  "inset-x": ["inset-inline", "left/right"],
  "inset-y": ["inset-block", "top/bottom"],
  "scroll-mx": ["scroll-margin-inline", "scroll-margin-left/right"],
  "scroll-my": ["scroll-margin-block", "scroll-margin-top/bottom"],
};
function axisReview(kind, name) {
  if (!kind.startsWith("utility")) return null;
  const normalized = name.startsWith("-") ? name.slice(1) : name;
  const root = Object.keys(axisSemantics).find(
    (value) => normalized === value || normalized.startsWith(`${value}-`),
  );
  if (!root) return null;
  const [upstream, wind] = axisSemantics[root];
  return `Tailwind ${upstream} uses logical axes; Wind ${wind} uses physical axes. Review horizontal and vertical writing modes.`;
}

function sourceEvidence(windSha, catalog, profile, upstream) {
  return {
    status: "source-inspected",
    windGitSha: windSha,
    windSpecVersion: catalog.specVersion,
    windSpecRevision: catalog.specRevision,
    referenceVersion: upstream.pin.version,
    referenceSourceTag: upstream.pin.tag,
    referenceObservedTagCommit: upstream.pin.observedTagCommit,
    referenceArtifactLink: upstream.pin.tagToArtifactLink,
    profileId: profile.profileId,
    profileVersion: profile.profileVersion,
    profileRevision: profile.profileRevision,
    caseIds: [],
    buildSha: null,
    browserEnvironment: null,
    reportId: null,
  };
}
export function makeInventory(
  upstream,
  catalog,
  profile,
  windSha,
  variantSource,
  docsEvidence,
  runtime,
) {
  if (windSha !== WIND_SOURCE_SHA) throw Error("Unreviewed Wind source SHA");
  if (
    runtime?.package !== "tailwindcss" ||
    runtime.version !== upstream.pin.version ||
    runtime.artifactSha256 !== "3673da9004404d12d4672bb5d002945319c2b3dea8c13ea022fdd33a3e260e62"
  )
    throw Error("Runtime keyset identity mismatch");
  if (!variantSource?.includes("pub struct VariantVocabulary"))
    throw Error("Wind variant source missing");
  const windStates = [
    ...(variantSource
      .match(/pub\(crate\) fn is_state[\s\S]*?\[([\s\S]*?)\]/)?.[1]
      .matchAll(/\"([^\"]+)\"/g) ?? []),
  ].map((m) => m[1]);
  const windPseudo = ["before", "after", "marker", "placeholder", "backdrop", "selection"];
  const variantMapping = (kind, name) => {
    if (!kind.startsWith("variant")) return null;
    if (
      windStates.includes(name) ||
      windPseudo.includes(name) ||
      ["pointer-coarse", "pointer-fine"].includes(name)
    )
      return { kind: "exact-parser", source: "crates/zudo-wind/src/variant.rs", wind: name };
    if (name === "dark")
      return {
        kind: "configured-parser",
        source: "crates/zudo-wind/src/variant.rs",
        wind: "dark",
        requirement: "dark enabled",
      };
    if (kind === "variant-compound" && ["group", "peer"].includes(name))
      return {
        kind: "parser-state-subset",
        source: "crates/zudo-wind/src/variant.rs",
        wind: `${name}-<one-of-wind-states>`,
        requirement:
          "State must be in Wind is_state allowlist; compound nesting and ordering unverified",
      };
    if (name === "max")
      return {
        kind: "configured-parser-pattern",
        source: "crates/zudo-wind/src/variant.rs",
        wind: "max-<configured-breakpoint>",
        requirement: "breakpoint name configured",
      };
    return null;
  };
  const names = [
    ...runtime.staticUtilities.map((name) => ["utility-static", name]),
    ...runtime.functionalUtilities.map((name) => ["utility-pattern", name]),
    ...runtime.variants.map(({ name, kind }) => [`variant-${kind}`, name]),
  ];
  const rows = names.map(([kind, name]) => {
    const id = `${kind}:${name}`;
    const constraint =
      kind === "utility-pattern"
        ? (upstream.functionalConstraints[name] ?? {
            domain: "runtime-registered-root-source-review",
            themeKeys: [],
            bareValueHandler: false,
            negative: name.startsWith("-"),
            fractions: false,
            staticValueKeys: [],
          })
        : null;
    const exact = kind === "utility-static" ? windExact(name, catalog) : null;
    const family =
      kind === "utility-pattern"
        ? catalog.entries.find(
            (e) =>
              e.root === name ||
              (name.startsWith("-") && e.root === name.slice(1) && e.negativePolicy === "allowed"),
          )
        : null;
    const candidate = kind.startsWith("utility") && nativeCandidate(name);
    const axisDifference = axisReview(kind, name);
    const special = Boolean(axisDifference && exact);
    const variant = variantMapping(kind, name);
    const mapping =
      variant ??
      (exact
        ? { kind: "exact-catalog", catalogId: exact.id, wind: name }
        : family
          ? { kind: "root-only-unverified", catalogId: family.id, wind: `${name}-<value>` }
          : null);
    return {
      id,
      upstream: {
        kind,
        name,
        representation:
          kind === "utility-pattern"
            ? `${name}-<value>`
            : kind === "variant-functional"
              ? name === "@"
                ? "@<container-name>"
                : `${name}-<value>`
              : kind === "variant-compound"
                ? `${name}-<variant>`
                : name,
        constraints: constraint,
      },
      windMapping: mapping,
      configurationRequirement:
        variant?.requirement ??
        (kind === "utility-pattern"
          ? constraint?.themeKeys.length
            ? `Upstream theme keys: ${constraint.themeKeys.join(", ")}; Wind named tokens use var(--zw-...) in zw-tokens; numeric spacingUnit is optional where relevant.`
            : "Pattern domain requires case-specific source review; no default token guarantee."
          : family?.tokenCategories?.length
            ? `Configure Wind ${family.tokenCategories.join(", ")} token for named values.`
            : "No configuration established for this row."),
      implementation: variant
        ? "source-inspected-variant-parser"
        : exact
          ? "source-inspected-exact-registration"
          : family
            ? "root-collision-unverified"
            : "not-established",
      disposition: special
        ? "reviewed-difference"
        : variant
          ? "candidate-mapping-untested"
          : kind.startsWith("variant")
            ? "variant-unmapped"
            : exact
              ? "candidate-mapping-untested"
              : family
                ? "requires-review"
                : candidate
                  ? "native-css-review-candidate"
                  : tracking(name)
                    ? "excluded-deferred"
                    : "unmapped-upstream-registration",
      semanticDifference: axisDifference,
      alternative: candidate
        ? "Review native CSS and accessibility behavior; no automatic adoption."
        : null,
      trackingIssue: tracking(name),
      evidence: sourceEvidence(windSha, catalog, profile, upstream),
    };
  });
  // Dynamic/template registrations are bounded source sites, not a claim that names are complete.
  return {
    schemaVersion: 1,
    kind: "wind-compatibility-inventory",
    upstreamPin: upstream.pin,
    upstreamSourceDigests: upstream.sourceDigests,
    runtimeDigest: digest(JSON.stringify(runtime)),
    catalogDigest: digest(JSON.stringify(catalog)),
    profileDigest: digest(JSON.stringify(profile)),
    wind: {
      gitSha: windSha,
      specVersion: catalog.specVersion,
      specRevision: catalog.specRevision,
      catalogEntryCount: catalog.entries.length,
    },
    windVariantSourceSha256: digest(variantSource),
    profile: {
      id: profile.profileId,
      version: profile.profileVersion,
      revision: profile.profileRevision,
    },
    limits: [
      "Literal registration membership is exhaustive for the pinned extracted set only.",
      "Template/loop registrations, plugin/custom utilities, token values, arbitrary values and variant compositions are open domains.",
      "Source inspection and catalog presence do not establish emitted CSS or browser parity.",
      "Wind variants have rank/order and configuration constraints; same spelling does not prove selector equivalence.",
    ],
    dynamicRegistrationSites: upstream.dynamicRegistrationSites,
    dynamicSourceSites: upstream.dynamicSourceSites.map((site) => ({
      ...site,
      accounting: upstream.finiteLoopSites.some(
        (loop) =>
          site.file === "utilities.ts" && site.line >= loop.line && site.line <= loop.endLine,
      )
        ? "finite-loop-expanded"
        : "unresolved-source-expression",
      constraint:
        "Unresolved source expression is an explicit extraction limit; no finite membership completeness claimed for this site.",
    })),
    sourceCrossCheck: {
      testsInspected: upstream.testsInspected,
      testExamples: upstream.testExamples,
      docs: docsEvidence,
      runtimeOnlyStatic: runtime.staticUtilities.filter(
        (name) => !upstream.staticUtilities.includes(name),
      ),
      runtimeOnlyFunctional: runtime.functionalUtilities.filter(
        (name) => !upstream.functionalUtilities.includes(name),
      ),
      sourceOnlyStatic: upstream.staticUtilities.filter(
        (name) => !runtime.staticUtilities.includes(name),
      ),
      sourceOnlyFunctional: upstream.functionalUtilities.filter(
        (name) => !runtime.functionalUtilities.includes(name),
      ),
      note: "Tests/docs inform semantics and missing-family review; neither is treated as a complete class list.",
    },
    rows,
    reviewedRowCount: rows.length,
    configuredPatterns: [
      {
        id: "pattern:breakpoint",
        upstream: "<configured-breakpoint>:<utility>",
        wind: "<configured-breakpoint>:<utility>",
        requirement: "matching breakpoint configured on each engine",
        disposition: "configuration-required-untested",
      },
      {
        id: "pattern:max-breakpoint",
        upstream: "max-<configured-breakpoint>:<utility>",
        wind: "max-<configured-breakpoint>:<utility>",
        requirement: "matching breakpoint configured on each engine",
        disposition: "configuration-required-untested",
      },
      {
        id: "pattern:min-breakpoint",
        upstream: "min-<configured-breakpoint>:<utility>",
        wind: null,
        requirement: "reference breakpoint configured; Wind parser has no min- prefix",
        disposition: "missing-mechanism",
      },
      {
        id: "pattern:group-state",
        upstream: "group-<state>:<utility>",
        wind: "group-<Wind is_state value>:<utility>",
        requirement: "Wind state allowlist only; selector semantics untested",
        disposition: "subset-unverified",
      },
      {
        id: "pattern:peer-state",
        upstream: "peer-<state>:<utility>",
        wind: "peer-<Wind is_state value>:<utility>",
        requirement: "Wind state allowlist only; selector semantics untested",
        disposition: "subset-unverified",
      },
      {
        id: "pattern:arbitrary",
        upstream: "<root>-[<arbitrary-value>]",
        wind: null,
        requirement: "Grammar varies by root and value type; not all strings are supported",
        disposition: "open-domain-review",
      },
    ],
    windEntries: catalog.entries.map((entry) => {
      const upstreamRowIds = rows
        .filter((row) => row.windMapping?.catalogId === entry.id)
        .map((row) => row.id);
      return {
        id: `wind:${entry.id}`,
        catalogId: entry.id,
        root: entry.root,
        upstreamRowIds,
        disposition: upstreamRowIds.length
          ? "upstream-registration-mapped-unverified"
          : "wind-only-or-upstream-unmapped-review",
        evidence: sourceEvidence(windSha, catalog, profile, upstream),
      };
    }),
    profileCases: profile.requiredCases.map((c) => ({
      id: `profile:${c.id}`,
      upstreamCandidate: c.candidate,
      configuration: c.configuration,
      windMapping: c.implementation === "implemented" ? c.candidate : null,
      implementation: c.implementation,
      disposition: c.disposition,
      semanticDifference: c.expectedContract,
      gapKind: ["unconfigured-p-4", "undeclared-palette"].includes(c.id)
        ? "missing-default-token"
        : null,
      reviewedDifferenceIds: c.reviewedDifferenceIds,
      evidence: { ...sourceEvidence(windSha, catalog, profile, upstream), caseIds: [c.id] },
    })),
    nativeCssReview: [
      "flex order/basis",
      "SVG fill/stroke presentation",
      "accessibility screen-reader helpers",
    ].map((family) => ({
      family,
      disposition: "review-candidate",
      approval: null,
      evidenceStatus: "source-inspected",
      alternative: "Native CSS and semantic HTML review required before any adoption.",
    })),
    noSingleCompatibilityPercentage: true,
  };
}

export function validateInventory(
  inventory,
  upstream,
  catalog,
  profile,
  variantSource,
  docsEvidence,
  runtime,
) {
  const fail = (reason) => {
    throw Error(reason);
  };
  if (inventory.schemaVersion !== 1 || inventory.kind !== "wind-compatibility-inventory")
    fail("Inventory schema");
  if (
    JSON.stringify(inventory.upstreamPin) !== JSON.stringify(upstream.pin) ||
    JSON.stringify(inventory.upstreamSourceDigests) !== JSON.stringify(upstream.sourceDigests)
  )
    fail("Stale upstream source");
  if (inventory.wind?.gitSha !== WIND_SOURCE_SHA) fail("Unreviewed Wind source SHA");
  if (inventory.runtimeDigest !== digest(JSON.stringify(runtime))) fail("Runtime keyset drift");
  if (
    inventory.catalogDigest !== digest(JSON.stringify(catalog)) ||
    inventory.profileDigest !== digest(JSON.stringify(profile))
  )
    fail("Catalog/profile content drift");
  if (
    inventory.wind.specVersion !== catalog.specVersion ||
    inventory.wind.specRevision !== catalog.specRevision ||
    inventory.wind.catalogEntryCount !== catalog.entries.length
  )
    fail("Stale Wind catalog");
  if (variantSource && inventory.windVariantSourceSha256 !== digest(variantSource))
    fail("Stale Wind variant parser");
  if (
    inventory.profile.id !== profile.profileId ||
    inventory.profile.version !== profile.profileVersion ||
    inventory.profile.revision !== profile.profileRevision
  )
    fail("Stale profile");
  const expected = new Set([
    ...runtime.staticUtilities.map((n) => `utility-static:${n}`),
    ...runtime.functionalUtilities.map((n) => `utility-pattern:${n}`),
    ...runtime.variants.map((v) => `variant-${v.kind}:${v.name}`),
  ]);
  if (
    expected.size !== inventory.rows.length ||
    inventory.reviewedRowCount !== inventory.rows.length
  )
    fail("Unclassified upstream member or row count drift");
  if (
    JSON.stringify(inventory.dynamicRegistrationSites) !==
    JSON.stringify(upstream.dynamicRegistrationSites)
  )
    fail("Dynamic registration count drift");
  if (
    JSON.stringify(
      inventory.dynamicSourceSites.map(({ accounting, constraint, ...site }) => site),
    ) !== JSON.stringify(upstream.dynamicSourceSites)
  )
    fail("Unaccounted dynamic source registration");
  if (
    docsEvidence &&
    JSON.stringify(inventory.sourceCrossCheck.docs) !== JSON.stringify(docsEvidence)
  )
    fail("Stale Wind docs cross-check");
  for (const [key, actual] of Object.entries({
    runtimeOnlyStatic: runtime.staticUtilities.filter(
      (name) => !upstream.staticUtilities.includes(name),
    ),
    runtimeOnlyFunctional: runtime.functionalUtilities.filter(
      (name) => !upstream.functionalUtilities.includes(name),
    ),
    sourceOnlyStatic: upstream.staticUtilities.filter(
      (name) => !runtime.staticUtilities.includes(name),
    ),
    sourceOnlyFunctional: upstream.functionalUtilities.filter(
      (name) => !runtime.functionalUtilities.includes(name),
    ),
  }))
    if (JSON.stringify(inventory.sourceCrossCheck[key]) !== JSON.stringify(actual))
      fail("Unclassified upstream source member");
  if (
    JSON.stringify(inventory.sourceCrossCheck.testExamples) !==
    JSON.stringify(upstream.testExamples)
  )
    fail("Stale upstream test cross-check");
  const profileIds = new Set(profile.requiredCases.map((c) => `profile:${c.id}`));
  if (
    inventory.profileCases?.length !== profileIds.size ||
    new Set(inventory.profileCases.map((c) => c.id)).size !== profileIds.size ||
    inventory.profileCases.some((c) => !profileIds.has(c.id))
  )
    fail("Stale profile cases");
  for (const c of profile.requiredCases) {
    const row = inventory.profileCases.find((item) => item.id === `profile:${c.id}`);
    if (
      row.upstreamCandidate !== c.candidate ||
      row.configuration !== c.configuration ||
      row.implementation !== c.implementation ||
      row.disposition !== c.disposition ||
      row.semanticDifference !== c.expectedContract ||
      JSON.stringify(row.reviewedDifferenceIds) !== JSON.stringify(c.reviewedDifferenceIds) ||
      JSON.stringify(row.evidence.caseIds) !== JSON.stringify([c.id])
    )
      fail(`Stale profile case ${c.id}`);
  }
  if (
    !Array.isArray(inventory.nativeCssReview) ||
    inventory.nativeCssReview.length !== 3 ||
    inventory.nativeCssReview.some(
      (r) => r.approval !== null || r.disposition !== "review-candidate",
    )
  )
    fail("Native CSS candidates not explicitly pending");
  if (
    inventory.configuredPatterns?.length !== 6 ||
    new Set(inventory.configuredPatterns.map((row) => row.id)).size !== 6 ||
    !inventory.configuredPatterns.some(
      (row) => row.id === "pattern:min-breakpoint" && row.disposition === "missing-mechanism",
    )
  )
    fail("Configured pattern accounting");
  if (
    inventory.windEntries?.length !== catalog.entries.length ||
    new Set(inventory.windEntries.map((row) => row.catalogId)).size !== catalog.entries.length
  )
    fail("Unaccounted Wind catalog entry");
  for (const entry of catalog.entries) {
    const row = inventory.windEntries.find((item) => item.catalogId === entry.id);
    const mapped = inventory.rows
      .filter((item) => item.windMapping?.catalogId === entry.id)
      .map((item) => item.id);
    if (
      !row ||
      row.id !== `wind:${entry.id}` ||
      row.root !== entry.root ||
      JSON.stringify(row.upstreamRowIds) !== JSON.stringify(mapped) ||
      row.disposition !==
        (mapped.length
          ? "upstream-registration-mapped-unverified"
          : "wind-only-or-upstream-unmapped-review")
    )
      fail(`Stale Wind reverse mapping ${entry.id}`);
  }
  const seen = new Set(),
    catalogIds = new Set(catalog.entries.map((e) => e.id));
  for (const row of inventory.rows) {
    if (seen.has(row.id) || !expected.has(row.id))
      fail(`Duplicate/unknown inventory row ${row.id}`);
    seen.add(row.id);
    if (
      row.id !== `${row.upstream.kind}:${row.upstream.name}` ||
      !row.disposition ||
      !row.configurationRequirement ||
      !row.evidence ||
      !row.implementation
    )
      fail(`Incomplete row ${row.id}`);
    if (
      row.windMapping &&
      (!row.windMapping.wind ||
        (row.windMapping.catalogId && !catalogIds.has(row.windMapping.catalogId)))
    )
      fail(`Stale mapping ${row.id}`);
    if (
      row.windMapping?.kind === "exact-catalog" &&
      windExact(row.upstream.name, catalog)?.id !== row.windMapping.catalogId
    )
      fail(`Incorrect exact mapping ${row.id}`);
    if (
      row.windMapping?.kind === "root-only-unverified" &&
      !catalog.entries.some(
        (entry) =>
          entry.id === row.windMapping.catalogId &&
          (entry.root === row.upstream.name ||
            (row.upstream.name.startsWith("-") &&
              entry.root === row.upstream.name.slice(1) &&
              entry.negativePolicy === "allowed")),
      )
    )
      fail(`Incorrect root mapping ${row.id}`);
    if (
      row.upstream.kind === "utility-pattern" &&
      JSON.stringify(row.upstream.constraints) !==
        JSON.stringify(
          upstream.functionalConstraints[row.upstream.name] ?? {
            domain: "runtime-registered-root-source-review",
            themeKeys: [],
            bareValueHandler: false,
            negative: row.upstream.name.startsWith("-"),
            fractions: false,
            staticValueKeys: [],
          },
        )
    )
      fail(`Stale pattern constraint ${row.id}`);
    if (row.implementation === "source-inspected-exact-registration" && !row.windMapping)
      fail(`Missing mapping ${row.id}`);
    if (
      row.evidence.status !== "source-inspected" ||
      row.evidence.windGitSha !== inventory.wind.gitSha ||
      row.evidence.referenceVersion !== upstream.pin.version ||
      row.evidence.reportId !== null ||
      row.evidence.browserEnvironment !== null
    )
      fail(`Invalid evidence ${row.id}`);
    if (
      row.trackingIssue &&
      !/^https:\/\/github\.com\/Takazudo\/zudo-front-builder\/issues\/\d+$/.test(row.trackingIssue)
    )
      fail(`Stale tracking syntax ${row.id}`);
  }
  if (!variantSource || !docsEvidence) fail("Canonical replay inputs missing");
  const canonical = makeInventory(
    upstream,
    catalog,
    profile,
    WIND_SOURCE_SHA,
    variantSource,
    docsEvidence,
    runtime,
  );
  if (JSON.stringify(inventory) !== JSON.stringify(canonical))
    fail("Inventory differs from reviewed source-derived contract");
  return {
    rows: inventory.rows.length,
    kinds: Object.fromEntries(
      [
        "utility-static",
        "utility-pattern",
        "variant-static",
        "variant-functional",
        "variant-compound",
      ].map((k) => [k, inventory.rows.filter((r) => r.upstream.kind === k).length]),
    ),
  };
}

export async function verifyRuntime(runtime, tarballPath, modulePath) {
  if (
    basename(modulePath) !== "lib.mjs" ||
    (await realpath(modulePath)) !== resolve(await realpath(dirname(modulePath)), "lib.mjs")
  )
    throw Error("Unverified compiler entrypoint");
  const tarball = await readFile(tarballPath);
  if (digest(tarball) !== runtime.artifactSha256) throw Error("Pinned npm tarball digest mismatch");
  const tar = gunzipSync(tarball);
  let manifest = null;
  const modules = new Map();
  for (let at = 0; at + 512 <= tar.length;) {
    const name = tar.toString("utf8", at, at + 100).replace(/\0.*$/, "");
    if (!name) break;
    const size = parseInt(
      tar
        .toString("ascii", at + 124, at + 136)
        .replace(/\0.*$/, "")
        .trim(),
      8,
    );
    if (!Number.isFinite(size)) throw Error("Invalid npm tarball");
    if (name === "package/package.json")
      manifest = JSON.parse(tar.toString("utf8", at + 512, at + 512 + size));
    if (/^package\/dist\/[A-Za-z0-9_.-]+\.mjs$/.test(name))
      modules.set(name.slice("package/dist/".length), tar.subarray(at + 512, at + 512 + size));
    at += 512 + Math.ceil(size / 512) * 512;
  }
  if (
    manifest?.name !== runtime.package ||
    manifest?.version !== runtime.version ||
    !modules.has("lib.mjs")
  )
    throw Error("Npm package identity mismatch");
  for (const [name, bytes] of modules) {
    const file = resolve(dirname(modulePath), name);
    if (
      !(await lstat(file)).isFile() ||
      (await realpath(file)) !== resolve(await realpath(dirname(file)), name)
    )
      throw Error(`Unverified compiler module ${name}`);
    const extracted = await readFile(file);
    if (digest(bytes) !== digest(extracted))
      throw Error(`Extracted compiler module ${name} differs from tarball`);
  }
  const { __unstable__loadDesignSystem } = await import(pathToFileURL(modulePath).href);
  const design = await __unstable__loadDesignSystem(runtime.input);
  const observed = {
    staticUtilities: [...design.utilities.keys("static")].sort(),
    functionalUtilities: [...design.utilities.keys("functional")].sort(),
    variants: [...design.variants.entries()]
      .map(([name, value]) => ({ name, kind: value.kind }))
      .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)),
  };
  for (const [key, actual] of Object.entries(observed))
    if (JSON.stringify(actual) !== JSON.stringify(runtime[key]))
      throw Error(`Runtime ${key} keyset drift`);
  return {
    staticUtilities: observed.staticUtilities.length,
    functionalUtilities: observed.functionalUtilities.length,
    variants: observed.variants.length,
  };
}

export async function loadJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}
