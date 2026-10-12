import { signal, type Signal } from "@takazudo/zfb/zudo-react";

export interface GfmOptions {
  strikethrough: boolean;
  table: boolean;
  autolinkLiteral: boolean;
  taskListItem: boolean;
  footnoteDefinition: boolean;
}

export type GfmKey = keyof GfmOptions;
export type GfmSignals = Record<GfmKey, Signal<boolean>>;

export const DEFAULT_GFM: GfmOptions = {
  strikethrough: true,
  table: true,
  autolinkLiteral: true,
  taskListItem: true,
  footnoteDefinition: true,
};

export function createGfmSignals(defaults: GfmOptions = DEFAULT_GFM): GfmSignals {
  return {
    strikethrough: signal(defaults.strikethrough),
    table: signal(defaults.table),
    autolinkLiteral: signal(defaults.autolinkLiteral),
    taskListItem: signal(defaults.taskListItem),
    footnoteDefinition: signal(defaults.footnoteDefinition),
  };
}

export function snapshotGfm(gfm: GfmSignals): GfmOptions {
  return {
    strikethrough: gfm.strikethrough.value,
    table: gfm.table.value,
    autolinkLiteral: gfm.autolinkLiteral.value,
    taskListItem: gfm.taskListItem.value,
    footnoteDefinition: gfm.footnoteDefinition.value,
  };
}
