// #3133 repro: no `.mdx` file imports this component. A correct fix must
// not let its mere presence next to `button.mdx` (in the same directory
// tree the collection walks) flip on workspace staging.
import { helper as sharedHelper } from "shared-utils";

export default function Orphan() {
  return <div>{sharedHelper()}</div>;
}
