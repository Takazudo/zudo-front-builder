import { PanelComposition } from "../../components/panel-composition";
export default function Page() {
  return (
    <html>
      <head>
        <title>Factory acceptance</title>
      </head>
      <body>
        <PanelComposition enabled={false} hasHost={true} suppressDefault={false} useMap={true} />
      </body>
    </html>
  );
}
