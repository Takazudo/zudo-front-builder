import type { Description } from "./description.js";
import type { ErrorBoundaryProps } from "./error-boundary.js";
import type { renderErrorBoundary } from "./error-boundary-client.js";

export const boundaryType = Symbol.for("@takazudo/zfb/zudo-react/ErrorBoundary");
export function boundaryRenderer(type: unknown): typeof renderErrorBoundary | undefined {
  return typeof type === "function"
    ? (type as { [boundaryType]?: typeof renderErrorBoundary })[boundaryType]
    : undefined;
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
