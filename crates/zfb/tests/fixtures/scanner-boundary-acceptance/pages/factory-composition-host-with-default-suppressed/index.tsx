import { PanelComposition } from "../../components/panel-composition";
export default function Page() {
  return (
    <html>
      <body>
        <PanelComposition enabled={true} hasHost={true} suppressDefault={true} useMap={false} />
      </body>
    </html>
  );
}
