import { Island } from "@takazudo/zfb";
import { NamedCounter } from "../components/mdx-counters";

function WrappedNamedCounter() {
  return (
    <>
      <Island when="load">
        <NamedCounter />
      </Island>
    </>
  );
}

export const components = { NamedCounter: WrappedNamedCounter };
