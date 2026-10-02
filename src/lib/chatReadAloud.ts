import type { ChatEntry } from "../hooks/useAgentChat";

/** Only completed replies are spoken; actions and tool progress are separate. */
export function getUnreadChatReplies(entries: ChatEntry[], afterId: number): ChatEntry[] {
  return entries.filter((entry) =>
    entry.id > afterId &&
    entry.kind === "final" &&
    entry.content.trim().length > 0
  );
}
