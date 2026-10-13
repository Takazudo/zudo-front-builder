import { createDescription, type Child, type Description } from "./description.js";
import type { ReadonlySignal } from "./reactive-types.js";

export interface ErrorBoundaryProps {
  children: () => Child;
  fallback: (error: unknown, reset: () => void) => Child;
  resetKey?: ReadonlySignal<unknown> | undefined;
}

export function ErrorBoundary(props: ErrorBoundaryProps): Description {
  return createDescription(ErrorBoundary, props as unknown as Record<string, unknown>);
}
const marker = Symbol.for("@takazudo/zfb/zudo-react/ErrorBoundary");
Object.defineProperty(ErrorBoundary, marker, { value: true });
export function isErrorBoundary(type: unknown): boolean {
  return typeof type === "function" && marker in type;
}
export function boundaryProps(description: Description): ErrorBoundaryProps {
  const props = description.props as unknown as ErrorBoundaryProps;
  if (typeof props.children !== "function" || typeof props.fallback !== "function")
    throw new TypeError("ZR_CHILD: ErrorBoundary requires children and fallback factories");
  if (props.resetKey !== undefined && props.resetKey?.$$zudoReactive !== "zudo-react.reactive.v1")
    throw new TypeError("ZR_CHILD: ErrorBoundary.resetKey requires a readable signal");
  return props;
}

// Only engine-created integrity failures bypass recovery. User error messages are
// never interpreted as protocol or ownership failures.
const integrityErrors = new WeakSet<object>();
export function integrity<T extends object>(error: T): T {
  integrityErrors.add(error);
  return error;
}
export function recoverable(error: unknown): boolean {
  return !(typeof error === "object" && error !== null && integrityErrors.has(error));
}
