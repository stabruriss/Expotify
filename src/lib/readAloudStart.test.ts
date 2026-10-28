import { describe, expect, test } from "bun:test";
import { readMayStart } from "./readAloudStart";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

function checks(device: () => Promise<boolean | null>, trackIsCurrent = async () => true) {
  const calls: string[] = [];
  return {
    calls,
    checks: {
      playbackOnThisMac: () => { calls.push("device"); return device(); },
      trackIsCurrent: () => { calls.push("track"); return trackIsCurrent(); },
    },
  };
}

describe("starting a read", () => {
  test("an automatic read is skipped while another device plays", async () => {
    const c = checks(async () => false);
    expect(await readMayStart(true, c.checks)).toBe(false);
    expect(c.calls).toEqual(["device"]);
  });

  test("an automatic read goes ahead on this Mac or when the device cannot be told", async () => {
    for (const device of [async () => true, async () => null, async () => { throw new Error("offline"); }]) {
      const c = checks(device);
      expect(await readMayStart(true, c.checks)).toBe(true);
      expect(c.calls).toEqual(["device", "track"]);
    }
  });

  test("a manual read never asks where Spotify plays", async () => {
    const c = checks(async () => false);
    expect(await readMayStart(false, c.checks)).toBe(true);
    expect(c.calls).toEqual(["track"]);
  });

  test("a read whose track ended does not start", async () => {
    const c = checks(async () => true, async () => false);
    expect(await readMayStart(true, c.checks)).toBe(false);
  });

  test("a track change during the device check stops the old read whatever the check returns", async () => {
    for (const answer of [true, null, new Error("offline")]) {
      const device = deferred<boolean | null>();
      let playing = "A";
      const c = checks(() => device.promise, async () => playing === "A");
      const starting = readMayStart(true, c.checks);
      // Spotify moves on while the check is out, and no new Insight replaces this read.
      playing = "B";
      if (answer instanceof Error) device.reject(answer);
      else device.resolve(answer);
      expect(await starting).toBe(false);
      expect(c.calls).toEqual(["device", "track"]);
    }
  });
});
