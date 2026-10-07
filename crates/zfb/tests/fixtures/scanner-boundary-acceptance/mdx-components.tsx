import { Island } from "@takazudo/zfb";
import Counter from "./components/mdx-counters";
import { Island as ZfbIsland } from "zfb";
import AliasCounter from "./components/mdx-alias-counters";

function WrappedCounter() {
  return (
    <>
      <Island when="load">
        <Counter />
      </Island>
    </>
  );
}

function WrappedAliasCounter() {
  return (
    <>
      <ZfbIsland when="load">
        <AliasCounter />
      </ZfbIsland>
    </>
  );
}

export default { Counter: WrappedCounter, AliasCounter: WrappedAliasCounter };
