const forbidden = new Set(["__proto__", "prototype", "constructor"]);

function fail(code: string, path: string, detail = ""): never {
  throw new TypeError(`${code} at ${path}${detail ? ` (${detail})` : ""}`);
}

function validate(value: unknown, path: string, ancestors: Map<object, string>): void {
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) fail("ZR_PROPS_NUMBER", path);
    return;
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
  if (
    !array &&
    Object.getPrototypeOf(record) !== Object.prototype &&
    Object.getPrototypeOf(record) !== null
  )
    fail("ZR_PROPS_OBJECT_KIND", path);
  ancestors.set(record, path);
  try {
    const descriptors = Object.getOwnPropertyDescriptors(record);
    for (const key of Reflect.ownKeys(descriptors)) {
      if (typeof key === "symbol") fail("ZR_PROPS_PROPERTY", path, "symbol key");
      if (array && key === "length") continue;
      const next = array ? `${path}[${key}]` : `${path}.${key}`;
      if (forbidden.has(key)) fail("ZR_PROPS_KEY", next);
      const descriptor = descriptors[key];
      if (!descriptor || !descriptor.enumerable || !("value" in descriptor))
        fail("ZR_PROPS_PROPERTY", next);
      if (array && (!/^(0|[1-9]\d*)$/.test(key) || Number(key) >= record.length))
        fail("ZR_PROPS_PROPERTY", next);
      validate(descriptor.value, next, ancestors);
    }
    if (array) {
      for (let index = 0; index < record.length; index++) {
        if (!Object.hasOwn(record, index)) fail("ZR_PROPS_UNDEFINED", `${path}[${index}]`);
      }
    }
  } finally {
    ancestors.delete(record);
  }
}

export function serializeProps(props: Record<string, unknown>): string {
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
  validate(payload, "props", new Map());
  return JSON.stringify(payload);
}

function emptyChildren(value: unknown): boolean {
  return (
    value == null ||
    typeof value === "boolean" ||
    (Array.isArray(value) && value.every(emptyChildren))
  );
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
  validate(parsed, "props", new Map());
  return parsed as Record<string, unknown>;
}
