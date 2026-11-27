import { Island } from "@takazudo/zfb";
import { Counter } from "./counter";

export function IslandSlot({ label }: { label: string }) {
  return (
    <Island when="load">
      <Counter label={label} />
    </Island>
  );
}
