// pnpm-lock.yaml mutations shared by the lock-fingerprint suite and the
// dependency-graph re-pin tests (#4218).
import { parseAllDocuments, stringify } from "yaml";

import { computeLockFingerprint } from "../../../research/v3-island-size/lock-fingerprint.mjs";

// Re-serializes both documents through plain JS, so a mutation is expressed on
// the parsed lock instead of on fragile text offsets.
export function mutate(lockText, change) {
  const documents = parseAllDocuments(lockText).map((document) => document.toJS());
  change(documents[0], documents[1]);
  return documents.map((document) => `---\n${stringify(document, { lineWidth: 0 })}`).join("\n");
}

export function renameVersion(project, name, from, to) {
  const matches = (value) => value === from || value.startsWith(`${from}(`);
  const swap = (value) => `${to}${value.slice(from.length)}`;
  const rekey = (records) => {
    for (const key of Object.keys(records)) {
      const prefix = `${name}@`;
      if (key.startsWith(prefix) && matches(key.slice(prefix.length))) {
        records[`${prefix}${swap(key.slice(prefix.length))}`] = records[key];
        delete records[key];
      }
    }
  };
  rekey(project.packages);
  rekey(project.snapshots);
  for (const importer of Object.values(project.importers)) {
    for (const section of ["dependencies", "devDependencies", "optionalDependencies"]) {
      const edge = importer[section]?.[name];
      if (edge && matches(edge.version)) edge.version = swap(edge.version);
    }
  }
  for (const snapshot of Object.values(project.snapshots)) {
    for (const section of ["dependencies", "optionalDependencies"]) {
      const value = snapshot?.[section]?.[name];
      if (value && matches(value)) snapshot[section][name] = swap(value);
    }
  }
}

export function addPackage(project, key, snapshot = {}) {
  project.packages[key] = { resolution: { integrity: `sha512-${key}==` } };
  project.snapshots[key] = snapshot;
}

// Test fixture — synthetic prior graph, not native evidence: `lockText` with
// one snapshot the fingerprint reaches moved (an added transitive peer), so
// the dependency fingerprint moves while the inventory keys stay the same.
export function moveReachableSnapshot(lockText, currentVersion) {
  const { inventory } = computeLockFingerprint({ lockText, currentVersion });
  const snapshot = inventory.snapshots[0];
  return {
    snapshot,
    lockText: mutate(lockText, (_environment, project) => {
      const record = project.snapshots[snapshot] ?? {};
      project.snapshots[snapshot] = {
        ...record,
        transitivePeerDependencies: [
          ...(record.transitivePeerDependencies ?? []),
          "test-fixture-peer",
        ],
      };
    }),
  };
}
