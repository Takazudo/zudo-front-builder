import { describe, expect, it } from "vite-plus/test";

import { poll } from "../../tests/sdk-alias-consumer/poll.mjs";

function makeClock() {
  let time = Date.parse("2026-10-08T00:00:00.000Z");
  return {
    now: () => time,
    sleep: async (ms) => {
      time += Math.max(ms, 1);
    },
  };
}

function clockOptions(clock, timeoutMs = 3) {
  return {
    timeoutMs,
    intervalMs: 1,
    now: clock.now,
    sleep: clock.sleep,
  };
}

describe("sdk alias consumer poll", () => {
  it("does not retain a thrown error as the timeout cause after a false result", async () => {
    const clock = makeClock();
    const transient = new Error("connection reset", { cause: new Error("socket closed") });
    let attempts = 0;

    const timeout = await poll(
      "dev page readiness",
      async () => {
        attempts += 1;
        if (attempts === 1) throw transient;
        return {
          ok: false,
          status: "HTTP 503",
          markers: [],
          note: "service is still starting",
        };
      },
      clockOptions(clock),
    ).catch((error) => error);

    expect(attempts).toBe(3);
    expect(timeout).toMatchObject({
      lastNonOkStatus: {
        status: "HTTP 503",
        note: "service is still starting",
      },
    });
    expect(timeout.cause).toBeUndefined();
    expect(timeout.message).toContain("Last thrown error: Error: connection reset");
    expect(timeout.message).toContain("cause: Error: socket closed");
  });

  it("retains probe statuses, markers, timestamps, and thrown error classes in order", async () => {
    const clock = makeClock();
    const steps = [
      () => {
        throw new TypeError("temporary fetch failure");
      },
      async () => ({
        ok: false,
        status: "HTTP 503",
        markers: ["Counter"],
        note: "waiting for the second island",
      }),
      async () => ({
        ok: true,
        status: "HTTP 200",
        markers: ["Counter", "NamedCounter"],
        note: "island markers ready",
      }),
    ];

    const result = await poll("island readiness", () => steps.shift()(), clockOptions(clock));
    expect(result.timeline).toEqual([
      expect.objectContaining({
        status: "threw",
        markers: [],
        thrownErrorClass: "TypeError",
      }),
      expect.objectContaining({
        status: "HTTP 503",
        markers: ["Counter"],
        thrownErrorClass: null,
      }),
      expect.objectContaining({
        status: "HTTP 200",
        markers: ["Counter", "NamedCounter"],
        thrownErrorClass: null,
      }),
    ]);

    const timestamps = result.timeline.map(({ timestamp }) => Date.parse(timestamp));
    expect(timestamps).toEqual([...timestamps].sort((left, right) => left - right));
    expect(result.timeline).toHaveLength(3);
  });

  it("propagates a fatal error immediately when the caller identifies it", async () => {
    const clock = makeClock();
    const fatal = new Error("dev server exited");
    let attempts = 0;
    const probe = async () => {
      attempts += 1;
      throw fatal;
    };

    await expect(
      poll("dev page readiness", probe, {
        ...clockOptions(clock),
        isFatal: (error) => error === fatal,
      }),
    ).rejects.toBe(fatal);
    expect(attempts).toBe(1);
  });
});
