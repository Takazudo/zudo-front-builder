/** Primitive route values copied without invoking author-defined coercion. */
export type StablePathScalar = string | number;
export type StablePathParameter = StablePathScalar | readonly StablePathScalar[];

export interface StablePathsEntry<Props = unknown> {
  readonly params: Readonly<Record<string, StablePathParameter>>;
  readonly props?: Props;
}

// Branding is private and weak: neither source objects nor abandoned snapshots
// are retained by the module. A structural lookalike/frozen array is not an opt-in.
const snapshots = new WeakSet<object>();

/** @internal Only the server router consumes this identity check. */
export function isStablePaths(value: object): boolean {
  return snapshots.has(value);
}

function invalid(location: string, reason: string): never {
  throw new TypeError(`[zfb-runtime] stablePaths(): ${location} ${reason}`);
}

function plainObject(value: unknown, location: string): asserts value is object {
  if (typeof value !== "object" || value === null) invalid(location, "must be a plain object");
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null)
    invalid(location, "must be a plain object");
}

function dataProperty(
  object: object,
  key: string,
  location: string,
): PropertyDescriptor | undefined {
  const descriptor = Object.getOwnPropertyDescriptor(object, key);
  if (descriptor && !("value" in descriptor)) invalid(location, "must not be an accessor");
  return descriptor;
}

function scalar(value: unknown, location: string): StablePathScalar {
  if (typeof value === "string" || (typeof value === "number" && Number.isFinite(value)))
    return value;
  return invalid(location, "must be a string or finite number");
}

function copyArray<T>(
  value: unknown,
  location: string,
  copy: (value: unknown, at: string) => T,
): T[] {
  if (!Array.isArray(value) || Object.getPrototypeOf(value) !== Array.prototype)
    invalid(location, "must be an ordinary array");
  // Read descriptors instead of element access/iteration: accessors on array
  // slots must fail without running them. Proxy introspection can still run
  // traps or throw; this helper does not promise side-effect-free proxy handling.
  const length = dataProperty(value, "length", `${location}.length`)!.value as number;
  const result: T[] = [];
  for (let i = 0; i < length; i++) {
    const at = `${location}[${i}]`;
    const slot = dataProperty(value, String(i), at);
    if (!slot) invalid(at, "must be an own data property (sparse arrays are unsupported)");
    result.push(copy(slot.value, at));
  }
  // Custom iteration/coercion hooks and non-index properties are outside the
  // ordinary-data contract. No getter or hook is invoked to detect them.
  if (Reflect.ownKeys(value).length !== length + 1)
    invalid(location, "must contain only array elements");
  return result;
}

/**
 * Explicitly capture an owned paths() snapshot at this call. Membership, order,
 * entry shells, params and parameter arrays are copied and frozen. Params use a
 * null prototype and own data properties (including a literal `__proto__` key).
 * Strings and finite numbers, including arrays of those values, are preserved;
 * `[]` and `""` remain distinct. Getters, sparse arrays, custom prototypes and
 * coercible object parameters are rejected. Proxy introspection may run traps.
 *
 * Props are retained by reference, not inspected or frozen. Their current
 * values are spread on each render and still take precedence over `params`.
 * Components receive frozen owned params unless props overrides that field.
 * Source mutation cannot change the snapshot; create a new snapshot for a new
 * generation. The router reuses successful validation only for these snapshots,
 * and each new router (including a dev reload) starts a fresh validation memo.
 * This does not cache rendering, getStaticProps, or no-paths SSR requests.
 */
export function stablePaths<
  Params extends { readonly [K in keyof Params]: StablePathParameter },
  Props = unknown,
>(
  entries: readonly { readonly params: Params; readonly props?: Props }[],
): readonly StablePathsEntry<Props>[] {
  const owned = copyArray(entries, "entries", (entry, at): StablePathsEntry<Props> => {
    plainObject(entry, at);
    const params = dataProperty(entry, "params", `${at}.params`)?.value as unknown;
    const props = dataProperty(entry, "props", `${at}.props`);
    plainObject(params, `${at}.params`);
    const ownedParams: Record<string, StablePathParameter> = Object.create(null);
    for (const key of Reflect.ownKeys(params)) {
      if (typeof key !== "string") invalid(`${at}.params`, "must have only string keys");
      const location = `${at}.params.${key}`;
      const descriptor = dataProperty(params, key, location)!;
      if (!descriptor.enumerable) invalid(location, "must be enumerable");
      const value: unknown = descriptor.value;
      const copied = Array.isArray(value)
        ? Object.freeze(copyArray(value, location, scalar))
        : scalar(value, location);
      Object.defineProperty(ownedParams, key, { value: copied, enumerable: true });
    }
    const ownedEntry: StablePathsEntry<Props> = {
      params: Object.freeze(ownedParams),
      ...(props ? { props: props.value as Props } : {}),
    };
    return Object.freeze(ownedEntry);
  });
  Object.freeze(owned);
  // No partial/failed preparation is ever branded.
  snapshots.add(owned);
  return owned;
}
