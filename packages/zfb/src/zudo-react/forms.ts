import { batch } from "./scheduler.js";
import { isReactive } from "./dom-bindings.js";
import { subscribe } from "./reactive.js";
import type { Signal } from "./reactive-types.js";
import type { RuntimeScope } from "./scope.js";
import { diagnostic, report, rootPath, type RootOptions } from "./root.js";

const compositionKey = Symbol.for("@takazudo/zfb/zudo-react/composition-v1");
const radioOwners = new WeakMap<
  Document,
  Set<{ root: Element; name: string; form: HTMLFormElement | null }>
>();
type Control = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;
type Model = Signal<string | boolean | null>;
export interface FormPlan {
  element: Element;
  props: Readonly<Record<string, unknown>>;
  path: string;
}
interface Binding {
  node: Control;
  model: Model;
  kind: "text" | "textarea" | "checkbox" | "select" | "radio";
  read(): string | boolean | null;
  write(value: string | boolean | null): void;
  event: "input" | "change";
  path: string;
}
export class FormError extends TypeError {
  constructor(
    readonly code: string,
    readonly path: string,
    readonly detail: string,
  ) {
    super(`${code}: ${detail} at ${path}`);
  }
}
function error(code: string, path: string, detail: string): never {
  throw new FormError(code, path, detail);
}
function writable(value: unknown, path: string): Model {
  if (
    !value ||
    typeof value !== "object" ||
    !("$$zudoReactive" in value) ||
    value.$$zudoReactive !== "zudo-react.reactive.v1" ||
    !("$$zudoWritable" in value) ||
    value.$$zudoWritable !== "zudo-react.writable.v1"
  )
    error("ZR_MODEL_READONLY", path, "model requires a writable signal");
  let proto = value;
  while (proto) {
    const descriptor = Object.getOwnPropertyDescriptor(proto, "value");
    if (descriptor) {
      if (typeof descriptor.set !== "function")
        error("ZR_MODEL_READONLY", path, "model requires a writable signal");
      return value as Model;
    }
    proto = Object.getPrototypeOf(proto);
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
// Text input: read/write string .value; listen to input.
function textAdapter(
  node: HTMLInputElement,
  path: string,
): Pick<Binding, "read" | "write" | "event"> {
  return {
    event: "input",
    read: () => node.value,
    write: (value) => {
      if (typeof value !== "string") error("ZR_MODEL_VALUE", path, "text requires string");
      if (node.value !== value) node.value = value;
    },
  };
}
// Textarea: read/write string .value; listen to input.
function textareaAdapter(
  node: HTMLTextAreaElement,
  path: string,
): Pick<Binding, "read" | "write" | "event"> {
  return {
    event: "input",
    read: () => node.value,
    write: (value) => {
      if (typeof value !== "string") error("ZR_MODEL_VALUE", path, "textarea requires string");
      if (node.value !== value) node.value = value;
    },
  };
}
// Checkbox: read/write boolean .checked; listen to change.
function checkboxAdapter(
  node: HTMLInputElement,
  path: string,
): Pick<Binding, "read" | "write" | "event"> {
  return {
    event: "change",
    read: () => node.checked,
    write: (value) => {
      if (typeof value !== "boolean") error("ZR_MODEL_VALUE", path, "checkbox requires boolean");
      if (node.checked !== value) node.checked = value;
    },
  };
}
// Single select: read/write string .value after options exist; listen to change.
function selectAdapter(
  node: HTMLSelectElement,
  path: string,
): Pick<Binding, "read" | "write" | "event"> {
  return {
    event: "change",
    read: () => node.value,
    write: (value) => {
      if (typeof value !== "string") error("ZR_MODEL_VALUE", path, "select requires string");
      if (!values(node).includes(value) && !(value === "" && node.selectedIndex === -1))
        error("ZR_MODEL_VALUE", path, `select has no option ${value}`);
      if (node.value !== value) node.value = value;
    },
  };
}
// Radio group: read selected option or null; write checked; listen to change.
function radioAdapter(node: HTMLInputElement): Pick<Binding, "read" | "write" | "event"> {
  return {
    event: "change",
    read: () => (node.checked ? node.value : null),
    write: (value) => {
      const checked = value === node.value;
      if (node.checked !== checked) node.checked = checked;
    },
  };
}
function adapterFor(
  kind: Binding["kind"],
  node: Control,
  path: string,
): Pick<Binding, "read" | "write" | "event"> {
  switch (kind) {
    case "text":
      return textAdapter(node as HTMLInputElement, path);
    case "textarea":
      return textareaAdapter(node as HTMLTextAreaElement, path);
    case "checkbox":
      return checkboxAdapter(node as HTMLInputElement, path);
    case "select":
      return selectAdapter(node as HTMLSelectElement, path);
    case "radio":
      return radioAdapter(node as HTMLInputElement);
  }
}
function values(node: HTMLSelectElement): string[] {
  return [...node.options].map((option) => option.value);
}
function optionValuesFor(group: Binding[]): string[] {
  return group.map((binding) => (binding.node as HTMLInputElement).value);
}
export function prepareForms(
  plans: FormPlan[],
  map: Map<Node, Node>,
  mode: "hydrate" | "mount",
  container: Element,
): Binding[] {
  const bindings: Binding[] = [];
  const groupByModel = new Map<Model, Binding[]>();
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
        const owners = radioOwners.get(node.ownerDocument);
        if (
          [...(owners ?? [])].some(
            (entry) =>
              entry.root !== container && entry.name === radio.name && entry.form === radio.form,
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
        kind: controlKind!,
        path,
        ...adapterFor(controlKind!, node as Control, path),
      };
      bindings.push(binding);
      const group = groupByModel.get(model) ?? [];
      group.push(binding);
      groupByModel.set(model, group);
    }
  }
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
      if (mode === "mount" && model.value !== null && !optionValues.includes(model.value as string))
        error("ZR_MODEL_VALUE", group[0]!.path, "radio model has no option");
      if (mode === "hydrate" && nodes.filter((node) => node.checked).length > 1)
        error("ZR_MODEL_CONFLICT", group[0]!.path, "multiple checked radios");
    } else if (
      mode === "hydrate" &&
      group.some((binding) => !Object.is(binding.read(), group[0]!.read()))
    )
      error("ZR_MODEL_CONFLICT", group[0]!.path, "shared model controls disagree");
    if (radios) {
      for (const binding of group) {
        const originalWrite = binding.write;
        binding.write = (value) => {
          if (value !== null && !optionValuesFor(group).includes(value as string))
            error("ZR_MODEL_VALUE", binding.path, "radio model has no option");
          originalWrite(value);
        };
      }
    }
  }
  return bindings;
}
export function reconcileForms(bindings: Binding[], mode: "hydrate" | "mount"): void {
  const groups = new Map<Model, Binding[]>();
  for (const binding of bindings)
    groups.set(binding.model, [...(groups.get(binding.model) ?? []), binding]);
  if (mode === "hydrate")
    batch(() => {
      for (const [model, group] of groups) {
        model.value =
          group[0]!.kind === "radio"
            ? (group.find((binding) => binding.read() !== null)?.read() ?? null)
            : group[0]!.read();
      }
    });
  else for (const [model, group] of groups) for (const binding of group) binding.write(model.value);
}
export function activateForms(
  bindings: Binding[],
  scope: RuntimeScope,
  cleanups: Array<() => void>,
  options: RootOptions,
  container: Element,
): void {
  const byForm = new Map<HTMLFormElement, Binding[]>();
  const owned =
    radioOwners.get(container.ownerDocument) ??
    new Set<{ root: Element; name: string; form: HTMLFormElement | null }>();
  radioOwners.set(container.ownerDocument, owned);
  const registrations: Array<{ root: Element; name: string; form: HTMLFormElement | null }> = [];
  for (const binding of bindings)
    if (binding.kind === "radio") {
      const node = binding.node as HTMLInputElement;
      const registration = { root: container, name: node.name, form: node.form };
      registrations.push(registration);
      owned.add(registration);
    }
  cleanups.push(() => {
    for (const registration of registrations) owned.delete(registration);
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
    node.addEventListener(binding.event, onInput);
    if (textual) {
      node.addEventListener("compositionstart", onStart);
      node.addEventListener("compositionend", onFinish);
      node.addEventListener("blur", onFinish);
    }
    cleanups.push(() => {
      node.removeEventListener(binding.event, onInput);
      if (textual) {
        node.removeEventListener("compositionstart", onStart);
        node.removeEventListener("compositionend", onFinish);
        node.removeEventListener("blur", onFinish);
      }
    });
    const subscription = subscribe(
      () => {
        const value = model.value;
        if (!composing && !unknown) {
          try {
            binding.write(value);
          } catch (failure) {
            if (!(failure instanceof FormError)) throw failure;
            report(
              options,
              diagnostic(
                options,
                failure.code,
                "update",
                `${rootPath(container)}${binding.path}`,
                "valid model update",
                failure.detail,
              ),
            );
          }
        }
      },
      {
        active: () => scope.active,
        report: (code, actual) =>
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
          ),
      },
    );
    subscription.run();
    cleanups.push(() => subscription.dispose());
    const form = node.form;
    if (form) byForm.set(form, [...(byForm.get(form) ?? []), binding]);
  }
  for (const [form, members] of byForm) {
    let active = true;
    const onReset = (event: Event) =>
      queueMicrotask(() => {
        if (!active || event.defaultPrevented) return;
        batch(() => {
          const groups = new Map<Model, Binding[]>();
          for (const binding of members)
            groups.set(binding.model, [...(groups.get(binding.model) ?? []), binding]);
          for (const [model, group] of groups)
            model.value =
              group[0]!.kind === "radio"
                ? (group.find((binding) => binding.read() !== null)?.read() ?? null)
                : group[0]!.read();
        });
      });
    form.addEventListener("reset", onReset);
    cleanups.push(() => {
      active = false;
      form.removeEventListener("reset", onReset);
    });
  }
}
