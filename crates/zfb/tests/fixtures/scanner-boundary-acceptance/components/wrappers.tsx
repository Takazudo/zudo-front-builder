import { Island as Boundary } from "@takazudo/zfb";

export function ForwardBoundary({ children }: { children: unknown }) {
  return <Boundary>{children as never}</Boundary>;
}

export function OuterForwardBoundary({ children }: { children: unknown }) {
  return <ForwardBoundary>{children as never}</ForwardBoundary>;
}

export function UnusedForwardBoundary({ children }: { children: unknown }) {
  return <Boundary>{children as never}</Boundary>;
}
