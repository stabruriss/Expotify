import type { ChatMessage, AgentChatResult, ToolOutcome } from "../types";

/**
 * What an entry is, independent of its visual role:
 * - "user": the user's message
 * - "final": a completed assistant reply (the only kind that should be read aloud)
 * - "tool_status": execution status from the Rust executor (success or failure)
 * - "notice": transport errors, cancellation
 */
export type ChatEntryKind = "user" | "final" | "tool_status" | "notice";

export interface ChatEntry {
  /** Stable per-session id; never reused. */
  id: number;
  role: "user" | "assistant" | "system";
  kind: ChatEntryKind;
  content: string;
  action?: string;
  trackName?: string;
  /** Executor outcomes behind this reply, in order (empty for pure conversation). */
  toolResults?: ToolOutcome[];
}

/** The backend calls the store needs; injected so the store can be tested without Tauri. */
export interface ChatTransport {
  agentChat(messages: ChatMessage[], requestId: string): Promise<AgentChatResult>;
  agentChatCancel(requestId: string): Promise<void>;
}

export interface SendHooks {
  onLikeChanged?: () => void;
}

export type ChatStore = ReturnType<typeof createChatStore>;

/**
 * Conversation state outside React, so it survives mount/unmount cycles (e.g. when the track
 * briefly goes null during search_and_play) and an in-flight request still lands in the
 * conversation. Every request carries its own id: only the request still awaited may add a
 * reply or clear the loading state; a cancelled or superseded one may only report what the
 * executor already did; a request from before a reset reports nothing into the new session.
 */
export function createChatStore(transport: ChatTransport) {
  const state = {
    entries: [] as ChatEntry[],
    history: [] as ChatMessage[],
    idCounter: 0,
    requestCounter: 0,
    /** Conversation generation; bumped by reset so older requests cannot write into it. */
    generation: 0,
    /** The request awaiting a reply (null = none). */
    activeRequest: null as string | null,
    /** The last request the user cancelled; its reply is dropped when it arrives. */
    cancelledRequest: null as string | null,
    listeners: new Set<() => void>(),
  };

  function publish() {
    state.listeners.forEach((listener) => listener());
  }

  function subscribe(listener: () => void) {
    state.listeners.add(listener);
    return () => {
      state.listeners.delete(listener);
    };
  }

  function append(entry: Omit<ChatEntry, "id">): ChatEntry {
    const full: ChatEntry = { id: ++state.idCounter, ...entry };
    state.entries = [...state.entries, full];
    publish();
    return full;
  }

  /**
   * Show what the executor actually did, one status line per outcome: every failure, every
   * track change, and (when no assistant reply describes them) every other success.
   */
  function showToolResults(toolResults: ToolOutcome[], verbose: boolean, hooks?: SendHooks) {
    for (const outcome of toolResults) {
      if (!outcome.ok) {
        append({ role: "system", kind: "tool_status", content: `Action failed: ${outcome.output}` });
      } else if (outcome.track_name) {
        append({ role: "system", kind: "tool_status", content: `Now playing: ${outcome.track_name}` });
      } else if (verbose) {
        append({ role: "system", kind: "tool_status", content: outcome.output });
      }
    }
    if (toolResults.some((o) => o.ok && (o.name === "like_current" || o.name === "unlike_current"))) {
      hooks?.onLikeChanged?.();
    }
    if (toolResults.some((o) => o.ok && o.name === "save_memory")) {
      localStorage.setItem("expotify_settings_memories_updated_at", String(Date.now()));
    }
  }

  /**
   * Record the assistant turn right after its own user message (located by identity, since
   * later insertions shift positions), so later turns see what the executor did even when
   * the model never summarised it. A user message that is no longer in the history (reset)
   * gets nothing.
   */
  function rememberAssistantTurn(userMessage: ChatMessage, message: string, toolResults: ToolOutcome[]) {
    const index = state.history.indexOf(userMessage);
    if (index < 0) return;
    const content = message || toolResults.map((o) => o.output).join("\n");
    if (!content) return;
    state.history.splice(index + 1, 0, {
      role: "assistant",
      content,
      ...(toolResults.length > 0 ? { tool_results: toolResults } : {}),
    });
  }

  async function sendMessage(text: string, hooks?: SendHooks) {
    const generation = state.generation;
    append({ role: "user", kind: "user", content: text });
    const userMessage: ChatMessage = { role: "user", content: text };
    state.history.push(userMessage);

    const requestId = `req-${++state.requestCounter}`;
    state.activeRequest = requestId;
    publish();
    const sameSession = () => state.generation === generation;
    const current = () =>
      sameSession() && state.activeRequest === requestId && state.cancelledRequest !== requestId;
    try {
      const result: AgentChatResult = await transport.agentChat(state.history, requestId);
      const toolResults = result.tool_results ?? [];
      // The conversation was reset meanwhile: nothing of this request belongs to the new
      // session (the executor's event log keeps the audit trail).
      if (!sameSession()) return;
      if (!current()) {
        if (toolResults.length > 0) {
          showToolResults(toolResults, true, hooks);
          rememberAssistantTurn(userMessage, "", toolResults);
        }
        return;
      }
      const message = result.response.message.trim();
      if (message) {
        append({
          role: "assistant",
          kind: "final",
          content: result.response.message,
          action: result.response.action,
          trackName: result.track_name ?? undefined,
          toolResults,
        });
      }
      rememberAssistantTurn(userMessage, message, toolResults);
      showToolResults(toolResults, !message, hooks);
      if (result.error) {
        append({ role: "system", kind: "notice", content: `Error: ${result.error}` });
      }
    } catch (e) {
      if (!current()) return;
      append({ role: "system", kind: "notice", content: `Error: ${e instanceof Error ? e.message : String(e)}` });
    } finally {
      if (state.activeRequest === requestId) {
        state.activeRequest = null;
        publish();
      }
    }
  }

  /** Ask the backend to stop the given request; it finishes any action already running. */
  function cancelBackend(requestId: string) {
    void transport.agentChatCancel(requestId).catch(() => {});
  }

  function cancel() {
    const requestId = state.activeRequest;
    if (!requestId) return;
    state.cancelledRequest = requestId;
    state.activeRequest = null;
    cancelBackend(requestId);
    append({ role: "system", kind: "notice", content: "Cancelled" });
  }

  /** Start a new conversation; an in-flight request is cancelled and can no longer write here. */
  function reset() {
    const requestId = state.activeRequest;
    state.generation += 1;
    state.entries = [];
    state.history = [];
    state.activeRequest = null;
    if (requestId) {
      state.cancelledRequest = requestId;
      cancelBackend(requestId);
    }
    publish();
  }

  return {
    subscribe,
    getEntries: () => state.entries,
    isLoading: () => state.activeRequest !== null,
    sendMessage,
    cancel,
    reset,
  };
}
