import "../styles/global.css";
import DynamicClass from "../components/dynamic-class";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Wind v3 functional confirmation</title>
      </head>
      <body>
        <main>
          <div id="aspect-shadow" class="aspect-16/9 shadow-card transition-shadow ease-gentle">
            W-A08 aspect and shadow
          </div>
          <div id="reserved" class="p-4 ordinary-card">
            Reserved authored class
          </div>
          <DynamicClass color="red" />
        </main>
      </body>
    </html>
  );
}
