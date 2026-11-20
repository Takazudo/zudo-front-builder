import { readFileSync } from "node:fs";
import { join } from "node:path";
import { HARNESS_DIR } from "./util.mjs";

export const SCHEMA_FILES = {
  profile: "profile.v1.json",
  receipt: "receipt.v1.json",
  record: "record.v1.json",
  "build-receipt": "build-receipt.v1.json",
  fixture: "tiny-fixture.v1.json",
};

export function loadSchema(name) {
  const file = SCHEMA_FILES[name];
  if (!file)
    throw new Error(`unknown schema ${name}; known: ${Object.keys(SCHEMA_FILES).join(", ")}`);
  return JSON.parse(readFileSync(join(HARNESS_DIR, "schema", file), "utf8"));
}

function typeOf(v) {
  if (v === null) return "null";
  if (Array.isArray(v)) return "array";
  if (Number.isInteger(v)) return "integer";
  return typeof v;
}

function typeMatches(v, t) {
  const actual = typeOf(v);
  if (t === "number") return actual === "number" || actual === "integer";
  return actual === t;
}

/** Validates the JSON Schema subset these schemas use. Returns error strings. */
export function validate(schema, value, path = "$") {
  const errors = [];
  if ("const" in schema && value !== schema.const)
    errors.push(`${path}: expected ${JSON.stringify(schema.const)}`);
  if (schema.enum && !schema.enum.includes(value))
    errors.push(`${path}: ${JSON.stringify(value)} not in ${JSON.stringify(schema.enum)}`);
  if (schema.type) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((t) => typeMatches(value, t))) {
      errors.push(`${path}: expected ${types.join("|")}, got ${typeOf(value)}`);
      return errors;
    }
  }
  if (typeof value === "string") {
    if (schema.minLength !== undefined && value.length < schema.minLength)
      errors.push(`${path}: shorter than ${schema.minLength}`);
    if (schema.pattern && !new RegExp(schema.pattern).test(value))
      errors.push(`${path}: does not match ${schema.pattern}`);
  }
  if (typeof value === "number" && schema.minimum !== undefined && value < schema.minimum)
    errors.push(`${path}: below ${schema.minimum}`);
  if (Array.isArray(value) && schema.items)
    value.forEach((v, i) => errors.push(...validate(schema.items, v, `${path}[${i}]`)));
  if (typeOf(value) === "object") {
    for (const key of schema.required ?? [])
      if (!(key in value)) errors.push(`${path}: missing ${key}`);
    const props = schema.properties ?? {};
    for (const [k, v] of Object.entries(value)) {
      if (props[k]) errors.push(...validate(props[k], v, `${path}.${k}`));
      else if (schema.additionalProperties === false)
        errors.push(`${path}: unexpected property ${k}`);
    }
  }
  return errors;
}

export function validateAs(name, value) {
  return validate(loadSchema(name), value);
}
