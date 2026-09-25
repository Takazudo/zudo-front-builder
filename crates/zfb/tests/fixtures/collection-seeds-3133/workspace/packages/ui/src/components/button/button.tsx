import "leftpad-priv";
import { helper as sharedHelper } from "shared-utils";

export default function Button() {
  return <button>{sharedHelper()}</button>;
}
