import { readFileSync } from "node:fs";

export const LINUX_BIRTH = "linux-proc-v1";

// Field 2 (comm) is quoted with parentheses but may itself contain ')' and newlines.
export function parseLinuxStat(text, pid) {
  const open = text.indexOf("(");
  const close = text.lastIndexOf(")");
  const fields = text
    .slice(close + 1)
    .trim()
    .split(/\s+/);
  if (
    open < 1 ||
    close <= open ||
    text.slice(0, open).trim() !== String(pid) ||
    !/^\d+$/.test(fields[19]) ||
    !/^\d+$/.test(fields[1]) ||
    !/^\d+$/.test(fields[2])
  )
    throw new Error("invalid /proc stat fields");
  return {
    pid,
    ppid: Number(fields[1]),
    pgid: Number(fields[2]),
    state: fields[0],
    comm: text.slice(open + 1, close),
    startTicks: fields[19],
  };
}

export function linuxBirth(pid, read = readFileSync) {
  if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error("invalid process ID");
  let stat;
  try {
    stat = read(`/proc/${pid}/stat`, "utf8");
  } catch (error) {
    if (error.code === "ENOENT" || error.code === "ESRCH") return null;
    return { pid, birthVersion: LINUX_BIRTH, birthUnverifiable: true, error: String(error) };
  }
  try {
    const bootId = read("/proc/sys/kernel/random/boot_id", "utf8").trim();
    if (!/^[\da-f]{8}(-[\da-f]{4}){3}-[\da-f]{12}$/i.test(bootId))
      throw new Error("invalid kernel boot_id");
    return { ...parseLinuxStat(stat, pid), birthVersion: LINUX_BIRTH, bootId };
  } catch (error) {
    return { pid, birthVersion: LINUX_BIRTH, birthUnverifiable: true, error: String(error) };
  }
}

export function sameLinuxBirth(a, b) {
  return (
    a?.birthVersion === LINUX_BIRTH &&
    b?.birthVersion === LINUX_BIRTH &&
    !a.birthUnverifiable &&
    !b.birthUnverifiable &&
    a.pid === b.pid &&
    typeof a.bootId === "string" &&
    a.bootId === b.bootId &&
    /^\d+$/.test(a.startTicks) &&
    a.startTicks === b.startTicks
  );
}
