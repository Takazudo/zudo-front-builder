import { Counter } from "../components/counter";
import { ThemeToggle } from "../components/theme-toggle";
import { Island } from "@takazudo/zfb";

export default function Home() {
  return (
    <main>
      <Island>
        <Counter />
      </Island>
      <Island>
        <ThemeToggle />
      </Island>
    </main>
  );
}
