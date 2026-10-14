import {
  ErrorBoundary,
  signal,
  type Child,
  type Description,
  type ErrorBoundaryProps,
  type ReadonlySignal,
} from "@takazudo/zfb/zudo-react";

const resetKey: ReadonlySignal<number> = signal(0);
const props: ErrorBoundaryProps = {
  children: () => <strong>healthy child</strong>,
  fallback: (error, reset) => {
    const caught: unknown = error;
    const retry: () => void = reset;
    // @ts-expect-error Caught values must be narrowed before accessing Error properties.
    void error.message;
    return <button on:click={retry}>{caught instanceof Error ? caught.message : "retry"}</button>;
  },
  resetKey,
};
const boundary: Description = ErrorBoundary(props);
const ownedChild: Child = boundary;
const boundaryJsx = (
  <ErrorBoundary
    fallback={(error, reset) => <button on:click={reset}>{String(error)}</button>}
    resetKey={resetKey}
  >
    {() => <span>structural child</span>}
  </ErrorBoundary>
);

// @ts-expect-error Children are evaluated inside the boundary through a factory.
ErrorBoundary({ children: <div />, fallback: () => <p>failed</p> });
// @ts-expect-error A fallback must be a factory receiving the caught value and reset callback.
ErrorBoundary({ children: () => <div />, fallback: <p>failed</p> });
// @ts-expect-error A fallback is required.
ErrorBoundary({ children: () => <div /> });
// @ts-expect-error resetKey is a readable signal, not a plain value.
ErrorBoundary({ children: () => <div />, fallback: () => null, resetKey: 1 });
// @ts-expect-error Ordinary unowned objects are not supported fallback children.
ErrorBoundary({ children: () => <div />, fallback: () => ({ message: "failed" }) });
// @ts-expect-error A narrower Error parameter cannot accept every caught value.
ErrorBoundary({ children: () => <div />, fallback: (error: Error) => error.message });

void [boundary, ownedChild, boundaryJsx];
