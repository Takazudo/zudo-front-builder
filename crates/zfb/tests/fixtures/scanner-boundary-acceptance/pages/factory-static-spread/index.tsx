import { Island } from "@takazudo/zfb";
import { FactoryCounter } from "../../components/factory-counter";
function createFactoryBoundary(deps: { FactoryCounter: typeof FactoryCounter }) {
  const Target = deps.FactoryCounter;
  return function FactoryBoundary() {
    return (
      <>
        <Island when="load">
          <Target />
        </Island>
      </>
    );
  };
}
const defaults = { FactoryCounter };
const FactoryBoundary = createFactoryBoundary({ ...defaults });
export default function Page() {
  return (
    <html>
      <head>
        <title>Factory acceptance</title>
      </head>
      <body>
        <FactoryBoundary />
      </body>
    </html>
  );
}
