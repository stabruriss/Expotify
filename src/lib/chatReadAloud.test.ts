import { describe, expect, test } from "bun:test";
import type { ChatEntry } from "../hooks/useAgentChat";
import { getUnreadChatReplies } from "./chatReadAloud";

type Entry = ChatEntry & { kind: "user" | "final" | "tool_status" | "notice" };

describe("chat auto read", () => {
  test("reads the assistant reply when Now playing is appended in the same update", () => {
    const entries: Entry[] = [
      { id: 1, role: "user", kind: "user", content: "播放 Fly Me to the Moon" },
      { id: 2, role: "assistant", kind: "final", action: "search_and_play", content: "给你播放 akiko 的版本。" },
      { id: 3, role: "system", kind: "tool_status", content: "Now playing: Fly Me to the Moon - akiko" },
    ];
    expect(getUnreadChatReplies(entries, 1)).toEqual([entries[1]]);
  });

  test("reads natural language replies regardless of the associated action", () => {
    for (const action of [undefined, "reply", "ask", "refuse", "like_current", "set_volume", "save_memory", "shuffle_liked"]) {
      const entry: Entry = { id: 1, role: "assistant", kind: "final", action, content: "好的。" };
      expect(getUnreadChatReplies([entry], 0)).toEqual([entry]);
    }
  });

  test("collects every new reply in order and does not repeat observed messages", () => {
    const entries: Entry[] = [
      { id: 1, role: "assistant", kind: "final", content: "之前的回复" },
      { id: 2, role: "user", kind: "user", content: "继续" },
      { id: 3, role: "assistant", kind: "final", content: "第一条新回复" },
      { id: 4, role: "system", kind: "tool_status", content: "Action failed: unavailable" },
      { id: 5, role: "assistant", kind: "final", content: "第二条新回复" },
    ];
    expect(getUnreadChatReplies(entries, 1)).toEqual([entries[2], entries[4]]);
    expect(getUnreadChatReplies(entries, 5)).toEqual([]);
  });

  test("ignores user messages, status lines and empty replies", () => {
    const entries: Entry[] = [
      { id: 1, role: "user", kind: "user", content: "你好" },
      { id: 2, role: "system", kind: "notice", content: "Cancelled" },
      { id: 3, role: "assistant", kind: "final", content: " \n " },
    ];
    expect(getUnreadChatReplies(entries, 0)).toEqual([]);
    expect(getUnreadChatReplies([], 0)).toEqual([]);
  });

  test("does not read tool progress or notices even when their role is assistant", () => {
    const entries: Entry[] = [
      { id: 1, role: "assistant", kind: "tool_status", content: "正在查找歌曲" },
      { id: 2, role: "assistant", kind: "notice", content: "请求已取消" },
      { id: 3, role: "assistant", kind: "final", content: "已找到这个版本。" },
    ];
    expect(getUnreadChatReplies(entries, 0)).toEqual([entries[2]]);
  });
});
