import { spawn } from "node:child_process";
import { closeSync, openSync } from "node:fs";

// macOS ships no flock(1) and Node has no flock binding, so a perl child holds
// flock(LOCK_EX|LOCK_NB) for as long as its stdin stays open. The lock dies
// with the controller (stdin EOF) or the helper; deleting the file proves nothing.
const PERL_HELPER = String.raw`
use strict; use Fcntl qw(:flock);
$| = 1;
my $path = shift @ARGV;
open(my $fh, ">>", $path) or do { print "ERROR open $!\n"; exit 4; };
if (!flock($fh, LOCK_EX | LOCK_NB)) { print "BUSY\n"; exit 3; }
print "LOCKED $$\n";
while (my $l = <STDIN>) {}
exit 0;
`;

export class LockBusyError extends Error {
  constructor(path) {
    super(`lock busy: ${path}`);
    this.code = "LOCK_BUSY";
  }
}

/** Acquire a nonblocking exclusive lock; rejects with LockBusyError on contention. */
export function acquireLock(path, { perl = "perl" } = {}) {
  closeSync(openSync(path, "a"));
  return new Promise((resolve, reject) => {
    const child = spawn(perl, ["-e", PERL_HELPER, path], { stdio: ["pipe", "pipe", "pipe"] });
    let out = "";
    let settled = false;
    const exited = new Promise((r) => child.once("exit", (code, signal) => r({ code, signal })));
    child.once("error", (e) => {
      if (!settled) {
        settled = true;
        reject(e);
      }
    });
    child.stdout.on("data", (d) => {
      out += d.toString();
      if (settled) return;
      if (out.startsWith("LOCKED")) {
        settled = true;
        resolve({
          path,
          helperPid: child.pid,
          async release() {
            child.stdin.end();
            return exited;
          },
          exited,
        });
      } else if (out.startsWith("BUSY")) {
        settled = true;
        child.stdin.end();
        reject(new LockBusyError(path));
      } else if (out.includes("\n")) {
        settled = true;
        reject(new Error(`lock helper failed: ${out.trim()}`));
      }
    });
    child.once("exit", (code) => {
      if (!settled) {
        settled = true;
        reject(new Error(`lock helper exited ${code} before reporting: ${out.trim()}`));
      }
    });
  });
}
