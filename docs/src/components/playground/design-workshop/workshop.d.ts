import type { SeedFiles, WorkshopSnapshot } from "./model.js";

export interface DesignWorkshop {
  getSnapshot(): WorkshopSnapshot;
  getSeedFiles(): SeedFiles;
  navigate(view?: "start" | "playground" | "starter", scroll?: boolean): void;
  dispose(): void;
}
export function mountDesignWorkshop(
  root: HTMLElement,
  options?: { initialState?: unknown },
): DesignWorkshop;
