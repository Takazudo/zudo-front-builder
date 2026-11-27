// Run under the pinned Node 26. A real owned group whose root shim exits on SIGINT
// before its child, which ignores INT and TERM: the child is reparented to this
// subreaper controller and dies under SIGKILL as a zombie libuv never reaps.
import { readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { snapshotOwned, spawnOwned, stopOwned } from "../lib/process-owner.mjs";
import { enableChildSubreaper } from "../lib/linux-subreaper.mjs";

await enableChildSubreaper();
const zfb =
  "import signal, time\n" +
  "for s in (signal.SIGINT, signal.SIGTERM): signal.signal(s, signal.SIG_IGN)\n" +
  "time.sleep(60)\n";
const handle = spawnOwned("sh", ["-c", `trap 'exit 0' INT; python3 -c '${zfb}' & wait`]);
const end = Date.now() + 5000;
while (handle.known.size < 2 && Date.now() < end) {
  snapshotOwned(handle, undefined, "linux");
  await delay(20);
}
const child = [...handle.known.values()].find((p) => p.pid !== handle.native.pid);
const report = await stopOwned(handle, { intWaitMs: 500, termWaitMs: 300, killWaitMs: 2000 });
const stat = (() => {
  try {
    return readFileSync(`/proc/${child.pid}/stat`, "utf8").split(") ")[1].split(" ");
  } catch {
    return null;
  }
})();
console.log(
  JSON.stringify({
    absent: report.absent,
    remaining: report.remaining.map((p) => p.pid),
    endings: report.endings,
    child: child.pid,
    childNow: stat && { state: stat[0], ppid: Number(stat[1]) },
    controller: process.pid,
    steps: report.steps.map((s) => [s.signal, s.target]),
  }),
);
