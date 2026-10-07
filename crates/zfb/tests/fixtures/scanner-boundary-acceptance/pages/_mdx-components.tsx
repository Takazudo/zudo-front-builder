import { Island } from "@takazudo/zfb";
import { NamedCounter } from "../components/mdx-counters";
import { Island as ZfbIsland } from "zfb";
import { AliasNamedCounter } from "../components/mdx-alias-counters";

function WrappedNamedCounter() {
  return (
    <>
      <Island when="load">
        <NamedCounter />
      </Island>
    </>
  );
}

function WrappedAliasNamedCounter() {
  return (
    <>
      <ZfbIsland when="load">
        <AliasNamedCounter />
      </ZfbIsland>
    </>
  );
}

export const components = {
  NamedCounter: WrappedNamedCounter,
  AliasNamedCounter: WrappedAliasNamedCounter,
};
