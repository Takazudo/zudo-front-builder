// Synthetic CSS decision inputs, never compiler output or a Tailwind package proof.
export const models = ["current", "stronger-relations", "utility-layer"];
export const placements = ["authored-before-utility", "authored-after-utility"];
export const layerOrders = {
  "utility-last": ["base", "components", "utilities"],
  "utility-middle": ["base", "utilities", "components"],
};
export const colors = {
  authored: "rgb(255, 0, 0)",
  utility: "rgb(0, 170, 0)",
  inactive: "rgb(0, 0, 255)",
};
const scenarios = [
  { name: "equal", specificity: "class" },
  { name: "type-strong", specificity: "type-class" },
  { name: "class-strong", specificity: "two-classes" },
  { name: "id-strong", specificity: "id" },
  { name: "base-normal", layer: "base" },
  { name: "components-normal", layer: "components" },
  { name: "authored-important", authoredImportant: true },
  { name: "base-important", layer: "base", authoredImportant: true },
  { name: "components-important", layer: "components", authoredImportant: true },
  { name: "utility-important", utilityImportant: true },
  { name: "both-important", authoredImportant: true, utilityImportant: true },
  { name: "both-base-important", layer: "base", authoredImportant: true, utilityImportant: true },
  {
    name: "both-components-important",
    layer: "components",
    authoredImportant: true,
    utilityImportant: true,
  },
];

function selector(model, relation, candidate) {
  const subject = `.${candidate}`;
  if (relation === "group") {
    return model === "stronger-relations"
      ? `${subject}:is(:where(.group):hover *)`
      : `:where(.group:hover) ${subject}`;
  }
  if (relation === "peer") {
    return model === "stronger-relations"
      ? `${subject}:is(:where(.peer):focus ~ *)`
      : `:where(.peer:focus) ~ ${subject}`;
  }
  if (relation === "dark") return `${subject}:where([data-theme="dark"], [data-theme="dark"] *)`;
  return subject;
}

// Deliberately explicit expected winners by scenario, independent of CSS parsing.
function winner(model, placement, layerOrder, relation, scenario) {
  const later = placement === "authored-before-utility" ? "utility" : "authored";
  if (scenario.utilityImportant) {
    if (!scenario.authoredImportant) return "utility";
    if (model === "utility-layer") {
      if (!scenario.layer) return "utility"; // Layered important beats unlayered important.
      return layerOrders[layerOrder].indexOf(scenario.layer) <
        layerOrders[layerOrder].indexOf("utilities")
        ? "authored"
        : "utility"; // Earlier layer wins important.
    }
    if (scenario.layer) return "authored";
    return model === "stronger-relations" && ["group", "peer"].includes(relation)
      ? "utility"
      : later;
  }
  if (scenario.authoredImportant) return "authored";
  if (model === "utility-layer") {
    if (!scenario.layer) return "authored"; // Unlayered normal beats every layer.
    return layerOrders[layerOrder].indexOf(scenario.layer) >
      layerOrders[layerOrder].indexOf("utilities")
      ? "authored"
      : "utility";
  }
  if (scenario.layer) return "utility";
  if (scenario.specificity === "id") return "authored";
  if (model === "stronger-relations" && ["group", "peer"].includes(relation)) {
    return scenario.specificity === "two-classes" ? later : "utility";
  }
  return ["type-class", "two-classes"].includes(scenario.specificity) ? "authored" : later;
}

export function createFixture(model, placement, layerOrder) {
  if (!models.includes(model) || !placements.includes(placement) || !layerOrders[layerOrder]) {
    throw new Error("Unknown cascade experiment axis");
  }
  const rows = [];
  const authored = [];
  const utility = [];
  const groups = {};
  for (const relation of ["plain", "group", "peer", "media", "dark"]) {
    groups[relation] = [];
    for (const scenario of scenarios) {
      const id = `${relation}-${scenario.name}`;
      const candidate = `u-${id}`;
      const authoredClass = `a-${id}`;
      let authoredSelector = `.${authoredClass}`;
      if (scenario.specificity === "type-class") authoredSelector = `span.${authoredClass}`;
      if (scenario.specificity === "two-classes") authoredSelector += ".strong";
      if (scenario.specificity === "id") authoredSelector = `#${id}`;
      let rule = `${authoredSelector} { border-top-color: ${colors.authored}${scenario.authoredImportant ? " !important" : ""}; }`;
      if (scenario.layer) rule = `@layer ${scenario.layer} { ${rule} }`;
      authored.push(rule);
      const utilitySelector = selector(model, relation, candidate);
      rule = `${utilitySelector} { border-top-color: ${colors.utility}${scenario.utilityImportant ? " !important" : ""}; }`;
      if (relation === "group") rule = `@media (hover: hover) { ${rule} }`;
      if (relation === "media") rule = `@media (min-width: 640px) { ${rule} }`;
      utility.push(rule);
      rows.push({
        id,
        relation,
        scenario: scenario.name,
        authoredSelector,
        utilitySelector,
        expected: colors[winner(model, placement, layerOrder, relation, scenario)],
      });
      groups[relation].push(
        `<span id="${id}" class="probe strong ${authoredClass} ${candidate}">${id}</span>`,
      );
    }
  }
  // Negative relation controls: self is not a group ancestor; peers only affect
  // following siblings, not previous siblings or nested descendants.
  const controls = [
    [
      "group-self",
      "group",
      '<span id="group-self" class="probe group u-control-group">group self</span>',
    ],
    [
      "peer-before",
      "peer",
      '<span id="peer-before" class="probe u-control-peer">previous sibling</span>',
    ],
    [
      "peer-nested",
      "peer",
      '<div><span id="peer-nested" class="probe u-control-peer">nested descendant</span></div>',
    ],
    [
      "peer-self",
      "peer",
      '<input id="peer-self" class="probe peer u-control-peer" aria-label="peer self">',
    ],
  ];
  utility.push(
    `@media (hover: hover) { ${selector(model, "group", "u-control-group")} { border-top-color: ${colors.utility}; } }`,
  );
  utility.push(
    `${selector(model, "peer", "u-control-peer")} { border-top-color: ${colors.utility}; }`,
  );
  const utilityCss =
    model === "utility-layer" ? `@layer utilities {\n${utility.join("\n")}\n}` : utility.join("\n");
  const stages =
    placement === "authored-before-utility"
      ? [authored.join("\n"), utilityCss]
      : [utilityCss, authored.join("\n")];
  const css =
    `@layer zw-reset, zw-tokens, zfb-hi, ${layerOrders[layerOrder].join(", ")};\n` +
    `@layer zw-reset { :where(.probe) { border-top: 2px solid ${colors.inactive}; display: block; } }\n` +
    stages.join("\n");
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Synthetic cascade experiment</title><style>${css}</style></head><body>
    <button id="neutral">neutral focus and hover</button>
    ${groups.plain.join("\n")}
    <div id="group-owner" class="group">${groups.group.join("\n")}</div>
    <div>${controls[1][2]}<input id="peer-owner" class="peer" aria-label="peer owner">${groups.peer.join("\n")}${controls[2][2]}</div>
    ${controls[0][2]}${controls[3][2]}
    ${groups.media.join("\n")}<div id="theme">${groups.dark.join("\n")}</div>
    </body></html>`;
  return { model, placement, layerOrder, css, html, rows, controls: controls.map(([id]) => id) };
}
