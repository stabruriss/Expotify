import { describe, expect, test } from "bun:test";
import { SpeechMusicPause } from "./speechMusicPause";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function music(isPlaying: boolean) {
  const calls: string[] = [];
  const commands = {
    getTrack: async () => { calls.push("state"); return { is_playing: isPlaying }; },
    pause: async () => { calls.push("pause"); isPlaying = false; },
    play: async () => { calls.push("play"); isPlaying = true; },
  };
  return { calls, commands, service: new SpeechMusicPause(commands) };
}

describe("music paused for speech", () => {
  test("never starts music that was already paused", async () => {
    const m = music(false); const owner = Symbol();
    await m.service.hold(owner);
    await m.service.release(owner);
    expect(m.calls).toEqual(["state"]);
  });

  test("restores playing music once when the last reader finishes", async () => {
    const m = music(true); const insight = Symbol(); const chat = Symbol();
    await m.service.hold(insight);
    await m.service.hold(chat);
    await m.service.release(insight);
    expect(m.calls).toEqual(["state", "pause"]);
    await m.service.release(chat);
    await m.service.release(chat);
    expect(m.calls).toEqual(["state", "pause", "play"]);
  });

  test("cancelled state lookup does not pause or play music", async () => {
    const state = deferred<{ is_playing: boolean }>(); const queried = deferred<void>();
    const calls: string[] = [];
    const service = new SpeechMusicPause({
      getTrack: () => { queried.resolve(); return state.promise; },
      pause: async () => { calls.push("pause"); }, play: async () => { calls.push("play"); },
    });
    const owner = Symbol(); const holding = service.hold(owner);
    await queried.promise;
    const releasing = service.release(owner);
    state.resolve({ is_playing: true });
    await holding; await releasing;
    expect(calls).toEqual([]);
  });

  test("cancellation during pause waits for pause before restoring", async () => {
    const paused = deferred<void>(); const started = deferred<void>(); const calls: string[] = [];
    const service = new SpeechMusicPause({
      getTrack: async () => ({ is_playing: true }),
      pause: async () => { calls.push("pause"); started.resolve(); await paused.promise; },
      play: async () => { calls.push("play"); },
    });
    const owner = Symbol(); const holding = service.hold(owner); await started.promise;
    const releasing = service.release(owner);
    await Promise.resolve(); expect(calls).toEqual(["pause"]);
    paused.resolve(); await holding; await releasing;
    expect(calls).toEqual(["pause", "play"]);
  });

  test("a new reader retains the pause while a cancelled reader releases", async () => {
    const m = music(true); const first = Symbol(); const next = Symbol();
    await m.service.hold(first);
    const releasing = m.service.release(first);
    const holding = m.service.hold(next);
    await releasing; await holding;
    expect(m.calls).toEqual(["state", "pause"]);
    await m.service.release(next);
    expect(m.calls).toEqual(["state", "pause", "play"]);
  });

  test("failed pause never triggers play and does not poison future sessions", async () => {
    const m = music(true); let fails = true;
    const service = new SpeechMusicPause({ ...m.commands, pause: async () => {
      if (fails) throw new Error("pause failed");
      await m.commands.pause();
    } });
    const first = Symbol();
    await expect(service.hold(first)).rejects.toThrow("pause failed");
    await service.release(first); expect(m.calls).toEqual(["state"]);
    fails = false; const next = Symbol();
    await service.hold(next); await service.release(next);
    expect(m.calls).toEqual(["state", "state", "pause", "play"]);
  });

  test("a reader starting during resume checks playback after resume finishes", async () => {
    const resumed = deferred<void>(); const started = deferred<void>();
    const calls: string[] = []; let playing = true;
    const service = new SpeechMusicPause({
      getTrack: async () => { calls.push("state"); return { is_playing: playing }; },
      pause: async () => { calls.push("pause"); playing = false; },
      play: async () => { calls.push("play"); started.resolve(); await resumed.promise; playing = true; },
    });
    const first = Symbol(); await service.hold(first);
    const releasing = service.release(first); await started.promise;
    const next = Symbol(); const holding = service.hold(next);
    resumed.resolve(); await releasing; await holding;
    expect(calls).toEqual(["state", "pause", "play", "state", "pause"]);
    expect(playing).toBe(false);
    await service.release(next); expect(playing).toBe(true);
  });
});
