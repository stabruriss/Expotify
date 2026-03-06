import { useState, useCallback, useEffect, useRef } from "react";
import type { ChatMessage, AgentChatResult } from "../types";
import { agentChat } from "../lib/tauri";

export interface ChatEntry {
  id: number;
  role: "user" | "assistant" | "system";
  content: string;
  action?: string;
  trackName?: string;
}

interface UseAgentChatOptions {
  onLikeChanged?: () => void;
}

// Module-level state — persists across React mount/unmount cycles
// (e.g. when track briefly goes null during search_and_play)
const chatStore = {
  entries: [] as ChatEntry[],
  history: [] as ChatMessage[],
  idCounter: 0,
};

export function useAgentChat(options?: UseAgentChatOptions) {
  const [entries, setEntries] = useState<ChatEntry[]>(chatStore.entries);
  const [loading, setLoading] = useState(false);
  const cancelledRef = useRef(false);

  // Sync from module store on mount (recover state after remount)
  useEffect(() => {
    setEntries(chatStore.entries);
  }, []);

  const sendMessage = useCallback(async (text: string) => {
    const userEntry: ChatEntry = {
      id: ++chatStore.idCounter,
      role: "user",
      content: text,
    };
    chatStore.entries = [...chatStore.entries, userEntry];
    setEntries(chatStore.entries);

    chatStore.history.push({ role: "user", content: text });

    cancelledRef.current = false;
    setLoading(true);
    try {
      const result: AgentChatResult = await agentChat(chatStore.history);

      // If cancelled while awaiting, discard the response
      if (cancelledRef.current) return;

      const assistantEntry: ChatEntry = {
        id: ++chatStore.idCounter,
        role: "assistant",
        content: result.response.message,
        action: result.response.action,
        trackName: result.track_name ?? undefined,
      };
      chatStore.entries = [...chatStore.entries, assistantEntry];
      setEntries(chatStore.entries);

      // Add assistant response to history for multi-turn
      chatStore.history.push({
        role: "assistant",
        content: result.response.message,
      });

      // Show execution result or error
      if (result.executed && result.track_name) {
        const sysEntry: ChatEntry = {
          id: ++chatStore.idCounter,
          role: "system",
          content: `Now playing: ${result.track_name}`,
        };
        chatStore.entries = [...chatStore.entries, sysEntry];
        setEntries(chatStore.entries);
      } else if (!result.executed && result.error) {
        const errEntry: ChatEntry = {
          id: ++chatStore.idCounter,
          role: "system",
          content: `Action failed: ${result.error}`,
        };
        chatStore.entries = [...chatStore.entries, errEntry];
        setEntries(chatStore.entries);
      }

      // Notify like status change if like/unlike was executed
      if (
        result.executed &&
        (result.response.action === "like_current" || result.response.action === "unlike_current") &&
        options?.onLikeChanged
      ) {
        options.onLikeChanged();
      }
    } catch (e) {
      if (cancelledRef.current) return;
      const errEntry: ChatEntry = {
        id: ++chatStore.idCounter,
        role: "system",
        content: `Error: ${e instanceof Error ? e.message : String(e)}`,
      };
      chatStore.entries = [...chatStore.entries, errEntry];
      setEntries(chatStore.entries);
    } finally {
      setLoading(false);
    }
  }, [options?.onLikeChanged]);

  const cancel = useCallback(() => {
    cancelledRef.current = true;
    setLoading(false);
    const sysEntry: ChatEntry = {
      id: ++chatStore.idCounter,
      role: "system",
      content: "Cancelled",
    };
    chatStore.entries = [...chatStore.entries, sysEntry];
    setEntries(chatStore.entries);
  }, []);

  const reset = useCallback(() => {
    chatStore.entries = [];
    chatStore.history = [];
    cancelledRef.current = false;
    setEntries([]);
  }, []);

  return { entries, loading, sendMessage, reset, cancel };
}
