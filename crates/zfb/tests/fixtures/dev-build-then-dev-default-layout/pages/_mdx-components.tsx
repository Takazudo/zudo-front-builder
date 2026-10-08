import { Island } from "@takazudo/zfb";
import { NamedCounter } from "../components/counter";

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
