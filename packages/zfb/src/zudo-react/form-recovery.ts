import type { FormRecovery, FormHost } from "./forms.js";
import type { RuntimeScope } from "./scope.js";
function within(scope: RuntimeScope, parents: readonly RuntimeScope[]): boolean {
  for (let current: RuntimeScope | undefined = scope; current; current = current.parent)
    if (parents.includes(current)) return true;
  return false;
}

// Optional policy uses the owning renderer's ledger and errors across package copies.
export function createFormRecovery(container: Element, host: FormHost): FormRecovery {
  const { controlOwners, error, batch } = host;
  return {
    prepare(plans, map, mode, replacing, parent) {
      const installed = [...(controlOwners.get(container.ownerDocument) ?? [])].filter(
        (entry) =>
          entry.root === container && entry.scope.active && !within(entry.scope, replacing),
      );
      // Fresh boundary controls still sit in the provisional tree during preflight.
      // Resolve their ancestor form through the hydration map for group identity.
      const plannedForms = new Map<string, Node>();
      for (const plan of plans) {
        const node = map.get(plan.element) as Element;
        if (node.localName === "form" && node.id && !plannedForms.has(node.id))
          plannedForms.set(node.id, node);
      }
      const form = (node: HTMLInputElement): Node | null => {
        const id = node.getAttribute("form");
        if (id !== null) {
          const target = plannedForms.get(id) ?? container.ownerDocument.getElementById(id);
          return target instanceof Element && target.localName === "form" ? target : null;
        }
        const ancestor = node.closest("form");
        if (ancestor) return map.get(ancestor) ?? ancestor;
        return parent instanceof Element
          ? parent.localName === "form"
            ? parent
            : parent.closest("form")
          : null;
      };
      return { installed, form };
    },
    validate(groupByModel, mode, { installed, form }) {
      for (const [model, group] of groupByModel) {
        for (const binding of group) {
          const radio = binding.node as HTMLInputElement;
          if (
            binding.kind === "radio" &&
            installed.some(
              (entry) =>
                entry.binding.kind === "radio" &&
                entry.name === radio.name &&
                entry.form === form(radio) &&
                entry.binding.model !== model,
            )
          )
            error("ZR_MODEL_CONFLICT", binding.path, "same-name radios use different models");
        }
        const related = installed
          .filter((entry) => entry.binding.model === model)
          .map((entry) => entry.binding);
        const complete = [...group, ...related];
        for (const binding of group) binding.group = complete;
        const radios = complete.every((binding) => binding.kind === "radio");
        if (complete.some((binding) => binding.kind === "radio") && !radios)
          error("ZR_MODEL_CONFLICT", group[0]!.path, "radio and non-radio share model");
        if (radios) {
          const nodes = complete.map((binding) => binding.node as HTMLInputElement);
          const names = new Set(
            nodes.map((node) => `${node.name}\u0000${node.getAttribute("form") ?? ""}`),
          );
          const optionValues = nodes.map((node) => node.value);
          if (
            names.size !== 1 ||
            new Set(nodes.map(form)).size !== 1 ||
            new Set(optionValues).size !== optionValues.length
          )
            error("ZR_MODEL_CONFLICT", group[0]!.path, "radio group membership or duplicate value");
          if (
            (mode === "mount" || group.every((binding) => binding.fresh)) &&
            model.value !== null &&
            !optionValues.includes(model.value as string)
          )
            error("ZR_MODEL_VALUE", group[0]!.path, "radio model has no option");
          if (
            mode === "hydrate" &&
            group.filter((binding) => !binding.fresh && (binding.node as HTMLInputElement).checked)
              .length > 1
          )
            error("ZR_MODEL_CONFLICT", group[0]!.path, "multiple checked radios");
        } else if (
          mode === "hydrate" &&
          group
            .filter((binding) => !binding.fresh)
            .some((binding, _, adopted) => !Object.is(binding.read(), adopted[0]!.read()))
        )
          error("ZR_MODEL_CONFLICT", group[0]!.path, "shared model controls disagree");
      }
    },
    members(binding) {
      return binding.live
        ? [...(controlOwners.get(container.ownerDocument) ?? [])]
            .filter(
              (entry) =>
                entry.root === container &&
                entry.scope.active &&
                entry.binding.model === binding.model,
            )
            .map((entry) => entry.binding)
        : binding.group;
    },
    reconcile(groups, mode) {
      if (mode === "hydrate")
        batch(() => {
          for (const [model, group] of groups) {
            const adopted = group.filter((binding) => !binding.fresh);
            if (adopted.length)
              model.value =
                adopted[0]!.kind === "radio"
                  ? (adopted.find((binding) => binding.read() !== null)?.read() ??
                    (group.some(
                      (binding) =>
                        binding.fresh && (binding.node as HTMLInputElement).value === model.value,
                    )
                      ? model.value
                      : null))
                  : adopted[0]!.read();
            for (const binding of group) if (binding.fresh) binding.write(model.value);
          }
        });
      else
        for (const [model, group] of groups)
          for (const binding of group) binding.write(model.value);
    },
  };
}
