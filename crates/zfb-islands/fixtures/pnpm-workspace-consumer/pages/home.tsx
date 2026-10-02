import { Counter, ThemeToggle } from "@takazudo/zfb-blog-islands";
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
