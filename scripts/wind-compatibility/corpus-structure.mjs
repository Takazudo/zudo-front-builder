import { digest } from "./reference.mjs";
import { parseCssStructure } from "./structure.mjs";

const prelude = "@layer zw-reset, zw-tokens, zfb-hi, base, components";
const reviewedContractsDigest = "03cfccf1401a4966e044d3be5706591117863e61939d7bbbb8935ed21b915000";

export function canonical(node) {
  if (node.kind === "declaration")
    return { kind: "declaration", name: node.name, value: node.value };
  if (node.kind === "statement") return { kind: "statement", text: node.text };
  return { kind: "rule", head: node.head, children: node.children.map(canonical) };
}

const declaration = (name, value) => ({ kind: "declaration", name, value });
const rule = (head, children) => ({ kind: "rule", head, children });

export function expectedWindTree(contract) {
  if (contract.empty === true) return [];
  const nodes = [{ kind: "statement", text: prelude }];
  if (Object.keys(contract.tokens).length)
    nodes.push(
      rule("@layer zw-tokens", [
        rule(
          ":root",
          Object.entries(contract.tokens).map(([name, value]) => declaration(name, value)),
        ),
      ]),
    );
  for (const [name, writes] of Object.entries(contract.registrations).sort(([a], [b]) =>
    a.localeCompare(b),
  ))
    nodes.push(
      rule(
        `@property ${name}`,
        Object.entries(writes).map(([property, value]) => declaration(property, value)),
      ),
    );
  for (const item of contract.rules) {
    const utility = rule(
      item.selector,
      item.writes.map(([name, value]) => declaration(name, value)),
    );
    nodes.push(item.media ? rule(`@media ${item.media}`, [utility]) : utility);
  }
  return nodes.map(canonical);
}

export function compareCorpusStructure(contract, windCss, referenceCss) {
  const wind = parseCssStructure(windCss).map(canonical);
  const reference = parseCssStructure(referenceCss).map(canonical);
  const expectedWind = expectedWindTree(contract);
  const windPass = digest(wind) === digest(expectedWind);
  const referenceTreeSha256 = digest(reference);
  const referencePass = referenceTreeSha256 === contract.referenceTreeSha256;
  return {
    pass: windPass && referencePass,
    windPass,
    referencePass,
    windTreeSha256: digest(wind),
    expectedWindTreeSha256: digest(expectedWind),
    referenceTreeSha256,
    expectedReferenceTreeSha256: contract.referenceTreeSha256,
    wind,
    reference,
  };
}

export function validateStructureContracts(contracts, manifest, empty) {
  if (digest(contracts) !== reviewedContractsDigest)
    throw Error("Reviewed structural expectations changed without policy review");
  if (
    contracts.schemaVersion !== 1 ||
    digest(Object.keys(contracts.cases)) !== digest(manifest.upstreamCases.map((row) => row.id))
  )
    throw Error("Structural contract membership changed");
  for (const row of manifest.upstreamCases) {
    const contract = contracts.cases[row.id];
    if (
      !contract ||
      !/^[0-9a-f]{64}$/.test(contract.referenceTreeSha256) ||
      digest(contract.rules.map((item) => item.candidate).sort()) !==
        digest([...new Set(row.candidates)].sort())
    )
      throw Error(`Structural contract incomplete ${row.id}`);
    if (
      !contract.rules.every(
        (item) =>
          item.selector &&
          Array.isArray(item.writes) &&
          item.writes.length &&
          new Set(item.writes.map(([name]) => name)).size === item.writes.length,
      )
    )
      throw Error(`Structural writes incomplete ${row.id}`);
  }
  if (digest(Object.keys(contracts.supplemental ?? {})) !== digest(empty.supplementalCaseIds))
    throw Error("Supplemental structural contract membership changed");
  for (const id of empty.supplementalCaseIds) {
    const contract = contracts.supplemental[id];
    const row = empty.cases.find((item) => item.caseId === id);
    if (
      !/^[0-9a-f]{64}$/.test(contract.referenceTreeSha256) ||
      digest(contract.rules.map((item) => item.candidate).sort()) !==
        digest(Object.keys(row.expectedWind.utilityWrites).sort())
    )
      throw Error(`Supplemental structural contract incomplete ${id}`);
    const serializedTokens = Object.fromEntries(
      Object.entries(row.expectedWind.tokenDeclarations).map(([name, value]) => [
        name,
        name === "--zw-spacing-unit" && value === "0.25rem" ? ".25rem" : value,
      ]),
    );
    if (
      digest(contract.tokens) !== digest(serializedTokens) ||
      contract.rules.some(
        (item) =>
          digest(Object.fromEntries(item.writes)) !==
          digest(row.expectedWind.utilityWrites[item.candidate]),
      )
    )
      throw Error(`Supplemental structural writes drift ${id}`);
  }
  return true;
}
