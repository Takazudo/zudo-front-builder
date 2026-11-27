// Run under the pinned Node 26 (node:ffi). A shell spawns `setsid sleep` and exits:
// with the controller as a subreaper the escaped sleep is reparented here and the
// census lists it; without it the sleep goes to init and nothing can see it.
import { spawn } from "node:child_process";
import { controllerChildren, enableChildSubreaper } from "../lib/linux-subreaper.mjs";

const enabled = process.argv[2] === "subreaper" ? await enableChildSubreaper() : null;
const shell = spawn("sh", ["-c", "setsid sleep 30 </dev/null >/dev/null 2>&1 & echo $!"], {
  stdio: ["ignore", "pipe", "ignore"],
});
let out = "";
shell.stdout.on("data", (d) => (out += d));
await new Promise((r) => shell.once("close", r));
const escaped = Number(out.trim());
const deadline = Date.now() + 2000;
let children = controllerChildren();
while (Date.now() < deadline && enabled && !children.includes(escaped)) {
  await new Promise((r) => setTimeout(r, 10));
  children = controllerChildren();
}
const unknown = children.filter((pid) => pid !== shell.pid);
process.kill(escaped, "SIGKILL");
console.log(JSON.stringify({ enabled, escaped, children, unknown }));
