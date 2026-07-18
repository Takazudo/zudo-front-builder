import path from "node:path";

// Issue #1701 (Wave 2 confirm pass): the virtual module's own source is an
// ABSOLUTE (not relative) import of the sibling host below — the exact
// shape the workspace tier of `remap_virtual_module_project_imports_to_shadow`
// (#1699/#1700) targets. `projectRoot` is `sub-packages/reroot-host`;
// `vmodule-sibling` is a workspace sibling, not a project descendant, so a
// relative spelling here would already have been relocation-stable — the
// point of this fixture is to prove the ABSOLUTE-import escape hatch gets
// closed too.
export default {
  name: "reroot-vmodule-preset",
  setup({ projectRoot, addVirtualModule }) {
    const siblingHost = path.join(projectRoot, "../vmodule-sibling/vmodule-host.ts");
    addVirtualModule(
      "virtual:reroot-vmodule-host",
      () => `import ${JSON.stringify(siblingHost)};\n`,
    );
  },
};
