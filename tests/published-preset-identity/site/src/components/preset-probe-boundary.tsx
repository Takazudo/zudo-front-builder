import type { Description } from "@takazudo/zfb/zudo-react";
import { Island } from "@takazudo/zfb";
import PresetProbe from "./preset-probe";

export function PresetProbeBoundary(): Description {
  return (
    <>
      <Island when="load">
        <PresetProbe />
      </Island>
    </>
  );
}
