import { Island } from "@takazudo/zfb";
import { PanelSlot } from "@fixture/widgets";
import { HostPanel } from "../../components/host-panel";

export default function HostOverride() {
  return (
    <html lang="en">
      <head>
        <title>Host panel override</title>
      </head>
      <body>
        <PanelSlot>
          <Island when="load">
            <HostPanel />
          </Island>
        </PanelSlot>
      </body>
    </html>
  );
}
