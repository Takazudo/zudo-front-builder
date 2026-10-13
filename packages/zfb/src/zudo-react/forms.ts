import { batch } from "./scheduler.js";
import { isReactive } from "./dom-bindings.js";
import { subscribe } from "./reactive.js";
import type { Signal } from "./reactive-types.js";
import type { RuntimeScope } from "./scope.js";
import { diagnostic, report, rootPath, type RootOptions } from "./root.js";

const compositionKey = Symbol.for("@takazudo/zfb/zudo-react/composition-v1");
// Capture every live model before a first boundary can appear in a later render.
const controlOwners = new WeakMap<Document, Set<ControlOwner>>();
export interface ControlOwner {
  root: Element;
  name: string;
  form: HTMLFormElement | null;
  binding: Binding;
  scope: RuntimeScope;
}
type Control = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;
export type Model = Signal<string | boolean | null>;
export interface FormPlan {
  element: Element;
  props: Readonly<Record<string, unknown>>;
  path: string;
}
export interface Binding {
  node: Control;
  model: Model;
  group?: Binding[];
  fresh: boolean;
  live: boolean;
  kind: "text" | "textarea" | "checkbox" | "select" | "radio";
  read(): string | boolean | null;
  write(value: string | boolean | null): void;
  event: "input" | "change";
  path: string;
}
export class FormError extends TypeError {
  declare readonly code: string;
  declare readonly path: string;
  declare readonly detail: string;
  constructor(code: string, path: string, detail: string) {
    super(`${code}: ${detail} at ${path}`);
    Object.assign(this, { code, path, detail });
  }
}

function error(code: string, path: string, detail: string): never {
  throw new FormError(code, path, detail);
}
function writable(value: unknown, path: string): Model {
  if (
    isReactive(value) &&
    "$$zudoWritable" in value &&
    value.$$zudoWritable === "zudo-react.writable.v1"
  ) {
    let proto: object | null = value;
    while (proto) {
      const descriptor = Object.getOwnPropertyDescriptor(proto, "value");
      if (descriptor) {
        if (typeof descriptor.set === "function") return value as Model;
        break;
      }
      proto = Object.getPrototypeOf(proto);
    }
  }
  return error("ZR_MODEL_READONLY", path, "model requires a writable signal");
}

function kind(node: Element, props: Readonly<Record<string, unknown>>): Binding["kind"] | null {
  if (node.localName === "textarea") return "textarea";
  if (node.localName === "select") return "select";
  if (node.localName !== "input") return null;
  const type = String(props.type ?? "text");
  if (type === "checkbox") return "checkbox";
  if (type === "radio") return "radio";
  return ["text", "search", "email", "url", "tel", "password"].includes(type) ? "text" : null;
}
// All adapters preserve native input/change events and avoid redundant writes.
function adapterFor(
  kind: Binding["kind"],
  control: Control,
  path: string,
): Pick<Binding, "read" | "write" | "event"> {
  const node = control as HTMLInputElement;
  const radio = kind === "radio";
  const checkbox = kind === "checkbox";
  const property = checkbox || radio ? "checked" : "value";
  return {
    event: kind === "text" || kind === "textarea" ? "input" : "change",
    read: () => (radio ? (node.checked ? node.value : null) : node[property]),
    write: (value) => {
      if (!radio) {
        if (typeof value !== (checkbox ? "boolean" : "string"))
          error("ZR_MODEL_VALUE", path, `${kind} requires ${checkbox ? "boolean" : "string"}`);
        if (
          kind === "select" &&
          !values(control as HTMLSelectElement).includes(value as string) &&
          !(value === "" && (control as HTMLSelectElement).selectedIndex === -1)
        )
          error("ZR_MODEL_VALUE", path, `select has no option ${value}`);
      }
      const next = radio ? value === node.value : value;
      if (node[property] !== next) (node as unknown as Record<string, unknown>)[property] = next;
    },
  };
}
function values(node: HTMLSelectElement): string[] {
  return [...node.options].map((option) => option.value);
}
function optionValuesFor(group: Binding[]): string[] {
  return group.map((binding) => (binding.node as HTMLInputElement).value);
}
function groupBindings(bindings: Binding[]): Map<Model, Binding[]> {
  const groups = new Map<Model, Binding[]>();
  for (const binding of bindings) {
    const group = groups.get(binding.model) ?? [];
    group.push(binding);
    groups.set(binding.model, group);
  }
  return groups;
}
export function prepareForms(
  plans: FormPlan[],
  map: Map<Node, Node>,
  mode: "hydrate" | "mount",
  container: Element,
  replacing: readonly RuntimeScope[] = [],
  parent?: Node | null,
): Binding[] {
  const recovery = formRecoveries.get(container);
  const state = recovery?.prepare(plans, map, mode, replacing, parent);
  const form = state?.form ?? ((node: HTMLInputElement) => node.form);
  const bindings: Binding[] = [];
  const namedRadios = new Map<string, Model>();
  for (const plan of plans) {
    const { props, path } = plan;
    const node = map.get(plan.element) as Element;
    const controlKind = kind(node, props);
    const modelValue = props.modelValue;
    const modelChecked = props.modelChecked;
    const hasValue = modelValue !== undefined;
    const hasChecked = modelChecked !== undefined;
    if (hasValue && hasChecked) error("ZR_MODEL_CONFLICT", path, "modelValue with modelChecked");
    if (
      (props.defaultValue !== undefined && props.value !== undefined) ||
      (props.defaultChecked !== undefined && props.checked !== undefined)
    )
      error("ZR_MODEL_CONFLICT", path, "default with static value or checked");
    if (
      (hasValue &&
        (props.defaultValue !== undefined ||
          (controlKind !== "radio" && props.value !== undefined))) ||
      (hasChecked && (props.defaultChecked !== undefined || props.checked !== undefined))
    )
      error("ZR_MODEL_CONFLICT", path, "model with default or static value");
    if (
      props.defaultValue !== undefined &&
      !["text", "textarea", "select"].includes(controlKind ?? "")
    )
      error("ZR_MODEL_UNSUPPORTED", path, "defaultValue on unsupported control");
    if (props.defaultChecked !== undefined && !["checkbox", "radio"].includes(controlKind ?? ""))
      error("ZR_MODEL_UNSUPPORTED", path, "defaultChecked on unsupported control");
    if (node.localName === "select" && (node as HTMLSelectElement).multiple)
      error("ZR_MODEL_UNSUPPORTED", path, "select[multiple]");
    if (hasValue || hasChecked) {
      for (const name of ["type", "multiple", "name", "form"])
        if (isReactive(props[name])) error("ZR_MODEL_UNSUPPORTED", path, `reactive ${name} shape`);
      if (
        !controlKind ||
        node.hasAttribute("contenteditable") ||
        (hasChecked && controlKind !== "checkbox") ||
        (hasValue && controlKind === "checkbox")
      )
        error("ZR_MODEL_UNSUPPORTED", path, node.localName);
      const model = writable(hasValue ? modelValue : modelChecked, path);
      const initial = model.value;
      if (
        controlKind === "checkbox"
          ? typeof initial !== "boolean"
          : controlKind === "radio"
            ? initial !== null && typeof initial !== "string"
            : typeof initial !== "string"
      )
        error("ZR_MODEL_VALUE", path, "model has wrong value type");
      if (controlKind === "select") {
        const options = [...(node as HTMLSelectElement).options].map((option) => option.value);
        if (new Set(options).size !== options.length)
          error("ZR_MODEL_CONFLICT", path, "duplicate select option value");
      }
      if (controlKind === "radio") {
        const radio = node as HTMLInputElement;
        if (!radio.name || typeof props.value !== "string")
          error("ZR_MODEL_UNSUPPORTED", path, "radio requires static name and value");
        const owners = controlOwners.get(node.ownerDocument);
        if (
          [...(owners ?? [])].some(
            (entry) =>
              entry.root !== container &&
              entry.binding.kind === "radio" &&
              entry.name === radio.name &&
              entry.form === form(radio),
          )
        )
          error("ZR_MODEL_CONFLICT", path, "same-name radio owned by another root");
        const key = `${radio.name}\u0000${radio.getAttribute("form") ?? ""}`;
        const prior = namedRadios.get(key);
        if (prior && prior !== model)
          error("ZR_MODEL_CONFLICT", path, "same-name radios use different models");
        namedRadios.set(key, model);
      }
      const binding: Binding = {
        node: node as Control,
        model,
        fresh: mode === "hydrate" && node === plan.element,
        live: false,
        kind: controlKind!,
        path,
        ...adapterFor(controlKind!, node as Control, path),
      };
      bindings.push(binding);
    }
  }
  const groupByModel = groupBindings(bindings);
  if (recovery) recovery.validate(groupByModel, mode, state!);
  else {
    for (const [model, group] of groupByModel) {
      const radios = group.every((binding) => binding.kind === "radio");
      if (group.some((binding) => binding.kind === "radio") && !radios)
        error("ZR_MODEL_CONFLICT", group[0]!.path, "radio and non-radio share model");
      if (radios) {
        const nodes = group.map((binding) => binding.node as HTMLInputElement);
        const names = new Set(
          nodes.map((node) => `${node.name}\u0000${node.getAttribute("form") ?? ""}`),
        );
        const optionValues = nodes.map((node) => node.value);
        if (
          names.size !== 1 ||
          new Set(nodes.map((node) => node.form)).size !== 1 ||
          new Set(optionValues).size !== optionValues.length
        )
          error("ZR_MODEL_CONFLICT", group[0]!.path, "radio group membership or duplicate value");
        if (
          mode === "mount" &&
          model.value !== null &&
          !optionValues.includes(model.value as string)
        )
          error("ZR_MODEL_VALUE", group[0]!.path, "radio model has no option");
        if (mode === "hydrate" && nodes.filter((node) => node.checked).length > 1)
          error("ZR_MODEL_CONFLICT", group[0]!.path, "multiple checked radios");
      } else if (
        mode === "hydrate" &&
        group.some((binding) => !Object.is(binding.read(), group[0]!.read()))
      )
        error("ZR_MODEL_CONFLICT", group[0]!.path, "shared model controls disagree");
    }
  }
  for (const group of groupByModel.values()) {
    if (group[0]!.kind === "radio") {
      for (const binding of group) {
        const originalWrite = binding.write;
        binding.write = (value) => {
          if (
            value !== null &&
            !optionValuesFor(formRecoveries.get(container)?.members(binding) ?? group).includes(
              value as string,
            )
          )
            error("ZR_MODEL_VALUE", binding.path, "radio model has no option");
          originalWrite(value);
        };
      }
    }
  }
  return bindings;
}
export function reconcileForms(
  bindings: Binding[],
  mode: "hydrate" | "mount",
  container: Element,
): void {
  const groups = groupBindings(bindings);
  const recovery = formRecoveries.get(container);
  if (recovery) return recovery.reconcile(groups, mode);
  if (mode === "hydrate") readModels(groups);
  else for (const [model, group] of groups) for (const binding of group) binding.write(model.value);
}
function readModels(groups: Map<Model, Binding[]>): void {
  batch(() => {
    for (const [model, group] of groups)
      model.value =
        group[0]!.kind === "radio"
          ? (group.find((binding) => binding.read() !== null)?.read() ?? null)
          : group[0]!.read();
  });
}

export function activateForms(
  bindings: Binding[],
  scope: RuntimeScope,
  cleanups: Array<() => void>,
  options: RootOptions,
  container: Element,
): void {
  const byForm = new Map<HTMLFormElement, Binding[]>();
  const owned = controlOwners.get(container.ownerDocument) ?? new Set<ControlOwner>();
  controlOwners.set(container.ownerDocument, owned);
  const registrations: ControlOwner[] = [];
  for (const binding of bindings) {
    const node = binding.node as HTMLInputElement;
    const registration = {
      root: container,
      name: node.name,
      form: (node.hasAttribute("form")
        ? node.ownerDocument.getElementById(node.getAttribute("form")!)
        : node.closest("form")) as HTMLFormElement | null,
      binding,
      scope,
    };
    registrations.push(registration);
    owned.add(registration);
  }
  cleanups.push(() => {
    for (const registration of registrations) {
      registration.binding.live = false;
      owned.delete(registration);
    }
  });
  for (const binding of bindings) {
    const { node, model } = binding;
    const textual = binding.kind === "text" || binding.kind === "textarea";
    let composing =
      textual && (node as unknown as Record<symbol, unknown>)[compositionKey] === "active";
    let unknown =
      textual &&
      node.ownerDocument.activeElement === node &&
      (node as unknown as Record<symbol, unknown>)[compositionKey] === undefined;
    const update = () => {
      model.value = binding.read();
    };
    const onInput = (event: Event) => {
      if (binding.kind === "radio" && !(node as HTMLInputElement).checked) return;
      if (textual && (event as InputEvent).isComposing) composing = true;
      update();
    };
    const onStart = () => {
      composing = true;
      unknown = false;
    };
    const onFinish = () => {
      composing = false;
      unknown = false;
      update();
    };
    const listeners: Array<[string, (event: Event) => void]> = [[binding.event, onInput]];
    if (textual)
      listeners.push(
        ["compositionstart", onStart],
        ["compositionend", onFinish],
        ["blur", onFinish],
      );
    for (const [name, handler] of listeners) node.addEventListener(name, handler);
    cleanups.push(() => {
      for (const [name, handler] of listeners) node.removeEventListener(name, handler);
    });
    const reportUpdate = (code: string, actual: string): void =>
      report(
        options,
        diagnostic(
          options,
          code,
          "update",
          `${rootPath(container)}${binding.path}`,
          "valid model update",
          actual,
        ),
      );
    const subscription = subscribe(
      () => {
        const value = model.value;
        if (!composing && !unknown) {
          try {
            binding.write(value);
          } catch (failure) {
            if (!(failure instanceof FormError)) throw failure;
            reportUpdate(failure.code, failure.detail);
          }
        }
      },
      {
        active: () => scope.active,
        report: reportUpdate,
      },
    );
    subscription.run();
    binding.live = true;
    cleanups.push(() => subscription.dispose());
    const form = node.form;
    if (form) byForm.set(form, [...(byForm.get(form) ?? []), binding]);
  }
  for (const [form, members] of byForm) {
    let active = true;
    const onReset = (event: Event) =>
      queueMicrotask(() => {
        if (!active || event.defaultPrevented) return;
        // A boundary owns only part of a native radio group. Read all current
        // members after the browser reset, not just this scope's unchecked part.
        // Keep ownership-local listeners so disposing a replaced attempt still
        // cancels its queued reset work.
        const recovery = formRecoveries.get(container);
        const current = recovery
          ? [
              ...new Set(members.flatMap((binding) => recovery.members(binding) ?? [binding])),
            ].filter((binding) => binding.node.form === form)
          : members;
        readModels(groupBindings(current));
      });
    form.addEventListener("reset", onReset);
    cleanups.push(() => {
      active = false;
      form.removeEventListener("reset", onReset);
    });
  }
}

const formRecoveries = new WeakMap<Element, FormRecovery>();
export interface FormState {
  form(node: HTMLInputElement): Node | null;
  installed: ControlOwner[];
}
export interface FormRecovery {
  prepare(
    plans: FormPlan[],
    map: Map<Node, Node>,
    mode: "hydrate" | "mount",
    replacing: readonly RuntimeScope[],
    parent?: Node | null,
  ): FormState;
  validate(groups: Map<Model, Binding[]>, mode: "hydrate" | "mount", state: FormState): void;
  members(binding: Binding): Binding[] | undefined;
  reconcile(groups: Map<Model, Binding[]>, mode: "hydrate" | "mount"): void;
}
export interface FormHost {
  controlOwners: typeof controlOwners;
  error: typeof error;
  batch: typeof batch;
}
export const formHost = { controlOwners, error, batch, recoveries: formRecoveries };
