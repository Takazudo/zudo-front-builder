import { digest } from "./reference.mjs";

const semantic = {
  "p-hsp-sm": { paddingTop: "padding-top", paddingRight: "padding-right", paddingBottom: "padding-bottom", paddingLeft: "padding-left", controlPadding: "padding-top" },
  "mx-hsp-sm": { marginLeft: "margin-left", marginRight: "margin-right", controlMargin: "margin-left" },
  "pt-vsp-md": { paddingTop: "padding-top", controlPaddingTop: "padding-top" },
  "my-vsp-md": { marginTop: "margin-top", marginBottom: "margin-bottom", controlMargin: "margin-top" },
  "bg-surface": { backgroundColor: "background-color", controlBackground: "background-color" },
  "text-surface": { color: "color", controlColor: "color" },
};
const schemas = Object.freeze({
  "semantic-initial-composed": semantic,
  "semantic-changed-composed": semantic,
  "numeric-spacing-removed": { "p-4": { padding: "padding-top", controlPadding: "padding-top", previousConfiguredPadding: null } },
  "palette-gray-500-opt-in": { "bg-gray-500": { backgroundColor: "background-color", controlBackground: "background-color" } },
});
const counts = Object.freeze({ "semantic-initial-composed": 17, "semantic-changed-composed": 17, "numeric-spacing-removed": 2, "palette-gray-500-opt-in": 2 });

export function supplementalProbePlan(row, sourceHtml) {
  const schema = schemas[row.caseId];
  if (!schema || digest(row.explicitCandidates) !== digest(Object.keys(schema)) ||
      digest(Object.keys(row.browserExpected ?? {})) !== digest(row.explicitCandidates) ||
      digest(Object.keys(row.browserTargets ?? {})) !== digest(row.explicitCandidates) ||
      digest(Object.keys(row.browserControls ?? {})) !== digest(row.explicitCandidates))
    throw Error(`Supplemental candidate/target/control membership changed: ${row.caseId}`);
  const plan = [];
  const sourceIds = new Set();
  for (const candidate of row.explicitCandidates) {
    const expected = row.browserExpected[candidate];
    const keys = Object.keys(schema[candidate]);
    if (!expected || Array.isArray(expected) || digest(Object.keys(expected)) !== digest(keys))
      throw Error(`Supplemental observable keys changed: ${row.caseId}/${candidate}`);
    const target = row.browserTargets[candidate];
    const control = row.browserControls[candidate];
    if (!target || !control || target === control || sourceIds.has(target) || sourceIds.has(control) ||
        sourceHtml.split(`id="${target}"`).length !== 2 || sourceHtml.split(`id="${control}"`).length !== 2)
      throw Error(`Supplemental target/control source membership changed: ${row.caseId}/${candidate}`);
    sourceIds.add(target);
    sourceIds.add(control);
    let targetCount = 0, controlCount = 0;
    for (const key of keys) {
      const value = expected[key];
      if (key === "previousConfiguredPadding") {
        if (row.caseId !== "numeric-spacing-removed" || value !== "1rem")
          throw Error(`Supplemental transition metadata changed: ${row.caseId}/${candidate}`);
        continue;
      }
      if (typeof value !== "string" || value.length === 0)
        throw Error(`Supplemental observation malformed: ${row.caseId}/${candidate}/${key}`);
      const role = key.startsWith("control") ? "control" : "target";
      if (role === "control") controlCount++;
      else targetCount++;
      plan.push({ candidate, key, property: schema[candidate][key], role, value, sourceTarget: target, sourceControl: control });
    }
    if (targetCount === 0 || controlCount === 0)
      throw Error(`Supplemental target/control observation missing: ${row.caseId}/${candidate}`);
  }
  if (plan.length !== counts[row.caseId] || plan.some((probe) => !probe.property))
    throw Error(`Supplemental observation count changed: ${row.caseId}`);
  return plan;
}
