const forbidden = new Set(["__proto__", "prototype", "constructor"]);

function fail(code: string, path: string, detail = ""): never {
  throw new TypeError(`${code} at ${path}${detail ? ` (${detail})` : ""}`);
}

function normalize(value: unknown, path: string, ancestors: Map<object, string>): unknown {
  if (value === null || typeof value === "string" || typeof value === "boolean") return value;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) fail("ZR_PROPS_NUMBER", path);
    return value;
  }
  if (value === undefined) fail("ZR_PROPS_UNDEFINED", path);
  if (typeof value === "function" || typeof value === "symbol" || typeof value === "bigint")
    fail(`ZR_PROPS_${typeof value}`.toUpperCase(), path);
  if (typeof value !== "object") fail("ZR_PROPS_VALUE", path);
  const record = value as object;
  if ("$$zudo" in record || "$$zudoReactive" in record) fail("ZR_PROPS_RUNTIME_VALUE", path);
  const ancestor = ancestors.get(record);
  if (ancestor !== undefined) fail("ZR_PROPS_CYCLE", path, `ancestor ${ancestor}`);
  const array = Array.isArray(record);
  const length = array ? (record as unknown[]).length : 0;
  if (
    !array &&
    Object.getPrototypeOf(record) !== Object.prototype &&
    Object.getPrototypeOf(record) !== null
  )
    fail("ZR_PROPS_OBJECT_KIND", path);
  ancestors.set(record, path);
  try {
    const descriptors = Object.getOwnPropertyDescriptors(record);
    const copy: Record<string, unknown> | unknown[] = array
      ? []
      : Object.create(Object.getPrototypeOf(record));
    for (const key of Reflect.ownKeys(descriptors)) {
      if (typeof key === "symbol") fail("ZR_PROPS_PROPERTY", path, "symbol key");
      if (array && key === "length") continue;
      const next = array ? `${path}[${key}]` : `${path}.${key}`;
      if (forbidden.has(key)) fail("ZR_PROPS_KEY", next);
      const descriptor = descriptors[key];
      if (!descriptor || !descriptor.enumerable || !("value" in descriptor))
        fail("ZR_PROPS_PROPERTY", next);
      if (array && (!/^(0|[1-9]\d*)$/.test(key) || Number(key) >= length))
        fail("ZR_PROPS_PROPERTY", next);
      if (!array && descriptor.value === undefined) continue;
      Object.defineProperty(copy, key, {
        value: normalize(descriptor.value, next, ancestors),
        enumerable: true,
        writable: true,
        configurable: true,
      });
    }
    if (array) {
      for (let index = 0; index < length; index++) {
        if (!Object.hasOwn(record, index)) fail("ZR_PROPS_UNDEFINED", `${path}[${index}]`);
      }
    }
    return copy;
  } finally {
    ancestors.delete(record);
  }
}

export function normalizeProps(props: Record<string, unknown>): Record<string, unknown> {
  if (props === null || Array.isArray(props) || typeof props !== "object")
    fail("ZR_PROPS_OBJECT_KIND", "props");
  const descriptors = Object.getOwnPropertyDescriptors(props);
  const child = descriptors.children;
  if (child) {
    if (!child.enumerable || !("value" in child)) fail("ZR_PROPS_PROPERTY", "props.children");
    if (!emptyChildren(child.value)) fail("ZR_PROPS_CHILDREN", "props.children");
    delete descriptors.children;
  }
  const payload = Object.defineProperties(Object.create(Object.getPrototypeOf(props)), descriptors);
  return normalize(payload, "props", new Map()) as Record<string, unknown>;
}

export function serializeProps(props: Record<string, unknown>): string {
  return JSON.stringify(normalizeProps(props));
}

export function emptyChildren(value: unknown, ancestors = new Set<object>()): boolean {
  if (value == null || typeof value === "boolean") return true;
  if (!Array.isArray(value) || ancestors.has(value)) return false;
  ancestors.add(value);
  try {
    const descriptors = Object.getOwnPropertyDescriptors(value);
    for (let index = 0; index < value.length; index++) {
      const descriptor = descriptors[index];
      if (descriptor && (!("value" in descriptor) || !emptyChildren(descriptor.value, ancestors)))
        return false;
    }
    return true;
  } finally {
    ancestors.delete(value);
  }
}

export function parseProps(json: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    fail("ZR_PROPS_JSON", "props");
  }
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== "object")
    fail("ZR_PROPS_OBJECT_KIND", "props");
  if (Object.hasOwn(parsed, "children")) fail("ZR_PROPS_CHILDREN", "props.children");
  normalize(parsed, "props", new Map());
  return parsed as Record<string, unknown>;
}
