import { useCallback, useSyncExternalStore } from "react";
import { agentChat, agentChatCancel } from "../lib/tauri";
import { createChatStore } from "../lib/chatStore";

export type { ChatEntry, ChatEntryKind } from "../lib/chatStore";

interface UseAgentChatOptions {
  onLikeChanged?: () => void;
}

// One conversation per app window, kept outside React (see createChatStore).
const store = createChatStore({ agentChat, agentChatCancel });

export function useAgentChat(options?: UseAgentChatOptions) {
  const entries = useSyncExternalStore(store.subscribe, store.getEntries);
  const loading = useSyncExternalStore(store.subscribe, store.isLoading);
  const onLikeChanged = options?.onLikeChanged;

  const sendMessage = useCallback(
    (text: string) => store.sendMessage(text, { onLikeChanged }),
    [onLikeChanged],
  );
  const cancel = useCallback(() => store.cancel(), []);
  const reset = useCallback(() => store.reset(), []);

  return { entries, loading, sendMessage, reset, cancel };
}
