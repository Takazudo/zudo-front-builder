import { PanelComposition } from "../../components/panel-composition";
export default function Page() {
  return (
    <html>
      <body>
        <PanelComposition enabled={true} hasHost={false} suppressDefault={true} useMap={false} />
      </body>
    </html>
  );
}
