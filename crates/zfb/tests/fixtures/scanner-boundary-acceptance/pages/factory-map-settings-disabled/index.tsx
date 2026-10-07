import { PanelComposition } from "../../components/panel-composition";
export default function Page() {
  return (
    <html>
      <body>
        <PanelComposition enabled={false} hasHost={true} suppressDefault={false} useMap={true} />
      </body>
    </html>
  );
}
