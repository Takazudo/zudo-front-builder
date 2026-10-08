import { Island } from "@takazudo/zfb";
import Counter from "./components/counter";

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
