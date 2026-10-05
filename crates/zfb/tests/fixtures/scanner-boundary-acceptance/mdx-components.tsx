import { Island } from "@takazudo/zfb";
import Counter from "./components/mdx-counters";

function WrappedCounter() {
  return (
    <>
      <Island when="load">
        <Counter />
      </Island>
    </>
  );
}

export default { Counter: WrappedCounter };
