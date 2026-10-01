// Structural input types used by SDK helpers that inspect authored children.
// The owned Island boundary validates descriptions and component identity
// at runtime before rendering.

/** Opaque description-shaped object accepted at the SDK boundary. */
export type VNodeObject = {
  readonly type: string | ((...args: unknown[]) => unknown) | (new (...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/** @deprecated Broad legacy input. Use Child from @takazudo/zfb/zudo-react for owned JSX. */
export type VNode =
  | string
  | number
  | boolean
  | null
  | undefined
  | bigint
  | VNodeArray
  | VNodeObject
  | object;

export interface VNodeArray extends ReadonlyArray<VNode> {}
