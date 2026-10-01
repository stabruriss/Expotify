import { describe, expect, test } from "bun:test";
import { createChatStore } from "./chatStore";
import type { AgentChatResult, ChatMessage, ToolOutcome } from "../types";

if (typeof localStorage === "undefined") {
  Object.assign(globalThis, { localStorage: { setItem() {} } });
}

interface PendingRequest {
  requestId: string;
  messages: ChatMessage[];
  resolve: (result: AgentChatResult) => void;
  reject: (error: unknown) => void;
}

function fakeTransport() {
  const pending: PendingRequest[] = [];
  const cancelled: string[] = [];
  const transport = {
    agentChat(messages: ChatMessage[], requestId: string) {
      return new Promise<AgentChatResult>((resolve, reject) => {
        pending.push({ requestId, messages: structuredClone(messages), resolve, reject });
      });
    },
    async agentChatCancel(requestId: string) {
      cancelled.push(requestId);
    },
  };
  return { pending, cancelled, transport };
}

const reply = (message: string, extra: Partial<AgentChatResult> = {}): AgentChatResult => ({
  response: { message, action: "reply" },
  executed: false,
  track_name: null,
  tool_results: [],
  ...extra,
});

const outcome = (name: string, ok: boolean, output: string, trackName?: string): ToolOutcome => ({
  call_id: `${name}-id`,
  name,
  ok,
  output,
  ...(ok ? {} : { error_code: "test_failure" }),
  ...(trackName ? { track_name: trackName } : {}),
});

const texts = (entries: { kind: string; content: string }[]) => entries.map((e) => `${e.kind}:${e.content}`);

describe("chat store", () => {
  test("late results of a cancelled request keep the request/reply order in history", async () => {
    const { pending, cancelled, transport } = fakeTransport();
    const store = createChatStore(transport);

    const first = store.sendMessage("request A");
    store.cancel();
    const second = store.sendMessage("request B");
    expect(cancelled).toEqual(["req-1"]);

    pending[0].resolve(reply("", { executed: true, error: "Cancelled", tool_results: [outcome("set_volume", true, "volume A applied")] }));
    await first;
    pending[1].resolve(reply("reply B"));
    await second;
    void store.sendMessage("request C");

    expect(pending[2].messages.map((m) => m.content)).toEqual([
      "request A", "volume A applied", "request B", "reply B", "request C",
    ]);
    expect(texts(store.getEntries())).toEqual([
      "user:request A",
      "notice:Cancelled",
      "user:request B",
      "tool_status:volume A applied",
      "final:reply B",
      "user:request C",
    ]);
  });

  test("reset cancels the pending request and drops its reply from the new session", async () => {
    const { pending, cancelled, transport } = fakeTransport();
    const store = createChatStore(transport);

    const request = store.sendMessage("request before reset");
    store.reset();
    expect(cancelled).toEqual(["req-1"]);
    expect(store.isLoading()).toBe(false);

    pending[0].resolve(reply("stale reply after reset", { tool_results: [outcome("set_volume", true, "Volume set to 10.")] }));
    await request;
    expect(store.getEntries()).toEqual([]);

    void store.sendMessage("fresh");
    expect(pending[1].messages.map((m) => m.content)).toEqual(["fresh"]);
  });

  test("a cancelled request cannot add a reply or clear the newer request's loading state", async () => {
    const { pending, transport } = fakeTransport();
    const store = createChatStore(transport);

    const first = store.sendMessage("request A");
    store.cancel();
    const second = store.sendMessage("request B");
    expect(store.isLoading()).toBe(true);

    pending[0].resolve(reply("late reply A"));
    await first;
    expect(store.isLoading()).toBe(true);
    expect(texts(store.getEntries())).not.toContain("final:late reply A");

    pending[1].resolve(reply("reply B"));
    await second;
    expect(store.isLoading()).toBe(false);
    expect(texts(store.getEntries())).toContain("final:reply B");
  });

  test("every outcome gets its own status line and side effects fire per outcome", async () => {
    const { pending, transport } = fakeTransport();
    const store = createChatStore(transport);
    let likeRefreshes = 0;
    const hooks = { onLikeChanged: () => { likeRefreshes += 1; } };

    const first = store.sendMessage("like this then play Song", hooks);
    pending[0].resolve(reply("Liked failed, Song is playing.", {
      executed: false,
      track_name: "Song",
      tool_results: [outcome("like_current", false, "Spotify is not connected."), outcome("search_and_play", true, "Now playing: Song", "Song")],
    }));
    await first;
    expect(texts(store.getEntries())).toEqual([
      "user:like this then play Song",
      "final:Liked failed, Song is playing.",
      "tool_status:Action failed: Spotify is not connected.",
      "tool_status:Now playing: Song",
    ]);
    expect(likeRefreshes).toBe(0);

    const second = store.sendMessage("now like it", hooks);
    pending[1].resolve(reply("Liked.", { executed: true, tool_results: [outcome("like_current", true, "Added the current song to Liked Songs.")] }));
    await second;
    expect(likeRefreshes).toBe(1);
  });

  test("a request that failed after actions shows the outcomes and the request error", async () => {
    const { pending, transport } = fakeTransport();
    const store = createChatStore(transport);

    const request = store.sendMessage("volume 30 then tell me a joke");
    pending[0].resolve(reply("", { executed: true, error: "provider boom", tool_results: [outcome("set_volume", true, "Volume set to 30.")] }));
    await request;
    expect(texts(store.getEntries())).toEqual([
      "user:volume 30 then tell me a joke",
      "tool_status:Volume set to 30.",
      "notice:Error: provider boom",
    ]);

    void store.sendMessage("next");
    const history = pending[1].messages;
    expect(history[1]).toEqual({
      role: "assistant",
      content: "Volume set to 30.",
      tool_results: [outcome("set_volume", true, "Volume set to 30.")],
    });
  });

  test("cancelling before anything ran shows only the cancellation", async () => {
    const { pending, transport } = fakeTransport();
    const store = createChatStore(transport);

    const request = store.sendMessage("play something");
    store.cancel();
    pending[0].reject(new Error("Cancelled"));
    await request;
    expect(texts(store.getEntries())).toEqual(["user:play something", "notice:Cancelled"]);
    expect(store.isLoading()).toBe(false);
  });
});
