export type ProofCounters = { mounts: number; cleanups: number; ticks: number };

declare global {
  interface Window {
    __builtSiteProof?: Record<string, ProofCounters>;
    __proofActivateAgain?: () => void;
  }
}

export function counters(name: string): ProofCounters {
  const all = (window.__builtSiteProof ??= {});
  return (all[name] ??= { mounts: 0, cleanups: 0, ticks: 0 });
}
