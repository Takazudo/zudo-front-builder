import { PanelComposition } from "../../components/panel-composition";
export default function Page() {
  return (
    <html>
      <head>
        <title>Factory acceptance</title>
      </head>
      <body>
        <PanelComposition enabled={true} hasHost={false} suppressDefault={false} useMap={false} />
      </body>
    </html>
  );
}
