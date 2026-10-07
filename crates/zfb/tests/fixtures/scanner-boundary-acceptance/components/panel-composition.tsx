import { Island } from "@takazudo/zfb";
import { DefaultPanelIsland, createChrome } from "@fixture/widgets";
import { HostPanel } from "./host-panel";

export function HostPanelIsland() {
  return (
    <>
      <Island when="load">
        <HostPanel />
      </Island>
    </>
  );
}
export const fixedPanels = { DefaultPanel: DefaultPanelIsland, HostPanel: HostPanelIsland };

export function PanelComposition({
  enabled,
  hasHost,
  suppressDefault,
  useMap = false,
}: {
  enabled: boolean;
  hasHost: boolean;
  suppressDefault: boolean;
  useMap?: boolean;
}) {
  const Chrome = createChrome({ enabled });
  return (
    <Chrome>
      {hasHost ? (
        useMap ? (
          <fixedPanels.HostPanel />
        ) : (
          <HostPanelIsland />
        )
      ) : suppressDefault ? null : useMap ? (
        <fixedPanels.DefaultPanel />
      ) : (
        <DefaultPanelIsland />
      )}
    </Chrome>
  );
}
