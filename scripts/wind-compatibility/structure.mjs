const prelude = "@layer zw-reset, zw-tokens, zfb-hi, base, components;";
const padding = (value) =>
  ["top", "right", "bottom", "left"].map((side) => `padding-${side}:${value};`).join("");

// Reviewed source-side expectations. These are independent of either compiler's output.
const contracts = {
  block: [".block{display:block;}", ".block{display:block;}"],
  hidden: [".hidden{display:none;}", ".hidden{display:none;}"],
  "inline-flex": [".inline-flex{display:inline-flex;}", ".inline-flex{display:inline-flex;}"],
  "mx-auto": [".mx-auto{margin-left:auto;margin-right:auto;}", ".mx-auto{margin-inline:auto;}"],
  "grid-cols-2": [
    ".grid-cols-2{grid-template-columns:repeat(2,minmax(0,1fr));}",
    ".grid-cols-2{grid-template-columns:repeat(2, minmax(0, 1fr));}",
  ],
  "p-0-empty": [`.p-0{${padding("0")}}`, ""],
  "p-0-mapped": [
    `.p-0{${padding("0")}}`,
    ":root, :host{--spacing-0:0px;}.p-0{padding:var(--spacing-0);}",
  ],
  "named-spacing": [
    `@layer zw-tokens{:root{--zw-color-surface:#123456;--zw-spacing-hsp-sm:17px;}}.p-hsp-sm{${padding("var(--zw-spacing-hsp-sm)")}}`,
    ":root, :host{--spacing-hsp-sm:17px;}.p-hsp-sm{padding:var(--spacing-hsp-sm);}",
  ],
  "named-color": [
    "@layer zw-tokens{:root{--zw-color-surface:#123456;--zw-spacing-hsp-sm:17px;}}.bg-surface{background-color:var(--zw-color-surface);}",
    ":root, :host{--color-surface:#123456;}.bg-surface{background-color:var(--color-surface);}",
  ],
  "unconfigured-p-4": ["", ""],
  "undeclared-palette": ["", ""],
  "configured-p-4": [
    `@layer zw-tokens{:root{--zw-spacing-unit:.25rem;}}.p-4{${padding("1rem")}}`,
    ":root, :host{--spacing:0.25rem;}.p-4{padding:calc(var(--spacing) * 4);}",
  ],
  "contents-gap": ["", ".contents{display:contents;}"],
  "hover-block": [
    "@media (hover: hover){.hover\\:block:hover{display:block;}}",
    ".hover\\:block{&:hover{@media (hover: hover){display:block;}}}",
  ],
  "breakpoint-block": [
    "@media (min-width: 640px){.sm\\:block{display:block;}}",
    ".sm\\:block{@media (width >= 640px){display:block;}}",
  ],
};

function withoutComments(css) {
  let result = "",
    quote = null;
  for (let at = 0; at < css.length; at++) {
    const char = css[at];
    if (quote) {
      result += char;
      if (char === "\\") result += css[++at] ?? "";
      else if (char === quote) quote = null;
    } else if (char === '"' || char === "'") {
      quote = char;
      result += char;
    } else if (char === "/" && css[at + 1] === "*") {
      const end = css.indexOf("*/", at + 2);
      if (end < 0) throw Error("Unterminated CSS comment");
      result += " ";
      at = end + 1;
    } else result += char;
  }
  if (quote) throw Error("Unterminated CSS string");
  return result;
}

export function parseCssStructure(css) {
  const text = withoutComments(css);
  let at = 0;
  function parse(nested) {
    const nodes = [];
    while (at < text.length) {
      while (/\s/.test(text[at] ?? "")) at++;
      if (text[at] === "}") {
        if (!nested) throw Error("Unmatched CSS closing brace");
        at++;
        return nodes;
      }
      if (at >= text.length) break;
      const begin = at;
      let quote = null,
        parens = 0;
      for (; at < text.length; at++) {
        const char = text[at];
        if (quote) {
          if (char === "\\") {
            at++;
            continue;
          }
          if (char === quote) quote = null;
        } else if (char === '"' || char === "'") quote = char;
        else if (char === "(") parens++;
        else if (char === ")") parens--;
        else if (parens === 0 && (char === ";" || char === "{" || char === "}")) break;
      }
      if (quote || parens !== 0) throw Error("Malformed CSS token");
      const token = text.slice(begin, at).trim();
      const delimiter = text[at];
      if (delimiter === "{") {
        at++;
        nodes.push({ kind: "rule", head: token, children: parse(true) });
      } else if (delimiter === ";") {
        at++;
        const declaration = /^([\w-]+):\s*([\s\S]+)$/.exec(token);
        if (declaration && !token.startsWith("@"))
          nodes.push({ kind: "declaration", name: declaration[1], value: declaration[2].trim() });
        else nodes.push({ kind: "statement", text: token });
      } else if (delimiter === "}") {
        if (token) throw Error("Unterminated CSS declaration");
      } else if (token) throw Error("Unterminated CSS token");
    }
    if (nested) throw Error("Unclosed CSS rule");
    return nodes;
  }
  return parse(false);
}

export function expectedStructure(caseId) {
  const pair = contracts[caseId];
  if (!pair) throw Error(`No structural contract for ${caseId}`);
  return {
    wind: parseCssStructure(pair[0] ? `${prelude}${pair[0]}` : ""),
    reference: parseCssStructure(pair[1]),
  };
}

function treeText(tree) {
  return JSON.stringify(tree);
}

function paths(tree, side, predicate) {
  const found = [];
  function visit(nodes, prefix) {
    nodes.forEach((node, index) => {
      const path = `${prefix}/${index}`;
      if (predicate(node)) found.push(path);
      if (node.children) visit(node.children, path);
    });
  }
  visit(tree, side);
  return found;
}

export function differenceOccurrences(caseId, wind, reference, diagnosticsPass) {
  const found = {};
  const add = (id, locations) => {
    if (locations.length) found[id] = locations;
  };
  const windText = treeText(wind),
    refText = treeText(reference);
  const windRules = (head) =>
    paths(wind, "wind", (node) => node.kind === "rule" && node.head === head);
  const refRules = (head) =>
    paths(reference, "reference", (node) => node.kind === "rule" && node.head === head);
  add(
    "wind-layer-order-prelude",
    !reference.some((node) => node.kind === "statement" && `${node.text};` === prelude)
      ? paths(wind, "wind", (node) => node.kind === "statement" && `${node.text};` === prelude)
      : [],
  );
  if (
    caseId === "mx-auto" &&
    windText.includes("margin-left") &&
    windText.includes("margin-right") &&
    refText.includes("margin-inline")
  )
    add("physical-logical-margin", windRules(".mx-auto"));
  if (caseId === "p-0-empty" && !refRules(".p-0").length)
    add("zero-without-theme", windRules(".p-0"));
  if (caseId === "p-0-mapped" && !windText.includes("--zw-spacing"))
    add(
      "zero-variable-indirection",
      paths(
        reference,
        "reference",
        (node) => node.kind === "declaration" && node.name === "--spacing-0",
      ),
    );
  if (
    ["p-0-mapped", "named-spacing", "configured-p-4"].includes(caseId) &&
    refText.includes('"name":"padding"')
  )
    add(
      "padding-expansion",
      paths(
        wind,
        "wind",
        (node) =>
          node.kind === "rule" &&
          node.children?.some(
            (child) => child.kind === "declaration" && child.name === "padding-top",
          ),
      ),
    );
  if (caseId === "named-spacing" && refText.includes("--spacing-hsp-sm"))
    add(
      "named-token-representation",
      paths(
        wind,
        "wind",
        (node) =>
          node.kind === "declaration" &&
          ["--zw-color-surface", "--zw-spacing-hsp-sm"].includes(node.name),
      ),
    );
  if (caseId === "named-color" && refText.includes("--color-surface"))
    add(
      "named-token-representation",
      paths(
        wind,
        "wind",
        (node) =>
          node.kind === "declaration" &&
          ["--zw-color-surface", "--zw-spacing-hsp-sm"].includes(node.name),
      ),
    );
  if (caseId === "configured-p-4" && refText.includes('"name":"--spacing"'))
    add(
      "numeric-scale-representation",
      paths(
        wind,
        "wind",
        (node) => node.kind === "declaration" && node.name === "--zw-spacing-unit",
      ),
    );
  if (
    ["unconfigured-p-4", "undeclared-palette"].includes(caseId) &&
    diagnosticsPass &&
    !wind.length &&
    !reference.length
  )
    add("missing-token-diagnostics", [`wind-diagnostic/${caseId}`]);
  if (["hover-block", "breakpoint-block"].includes(caseId) && reference[0]?.head?.startsWith("."))
    add(
      "variant-nesting",
      paths(wind, "wind", (node) => node.kind === "rule" && node.head.startsWith("@media")),
    );
  return found;
}

export function observedDifferenceIds(caseId, wind, reference, diagnosticsPass) {
  return Object.keys(differenceOccurrences(caseId, wind, reference, diagnosticsPass)).sort();
}

export function compareStructure(
  caseId,
  windCss,
  referenceCss,
  expectedDifferenceIds,
  diagnosticsPass,
) {
  const actual = { wind: parseCssStructure(windCss), reference: parseCssStructure(referenceCss) };
  const expected = expectedStructure(caseId);
  const occurrences = differenceOccurrences(caseId, actual.wind, actual.reference, diagnosticsPass);
  const differences = Object.keys(occurrences).sort();
  const expectedIds = [...expectedDifferenceIds].sort();
  const occurrenceCounts = Object.fromEntries(
    [...new Set([...differences, ...expectedIds])].sort().map((id) => [
      id,
      {
        expectedCount: expectedIds.includes(id)
          ? id === "named-token-representation" && ["named-spacing", "named-color"].includes(caseId)
            ? 2
            : 1
          : 0,
        observedCount: occurrences[id]?.length ?? 0,
        paths: occurrences[id] ?? [],
      },
    ]),
  );
  return {
    pass:
      treeText(actual.wind) === treeText(expected.wind) &&
      treeText(actual.reference) === treeText(expected.reference) &&
      treeText(differences) === treeText(expectedIds) &&
      Object.values(occurrenceCounts).every((entry) => entry.expectedCount === entry.observedCount),
    actual,
    expected,
    observedDifferenceIds: differences,
    expectedDifferenceIds: expectedIds,
    differenceOccurrences: occurrenceCounts,
  };
}

export function expectedExtractionStructure(candidates, engine) {
  const nodes = [];
  if (engine === "wind" && candidates.length) nodes.push(parseCssStructure(prelude)[0]);
  for (const candidate of candidates) {
    const contract = expectedStructure(candidate);
    nodes.push(...contract[engine].filter((node) => node.kind !== "statement"));
  }
  return nodes;
}
