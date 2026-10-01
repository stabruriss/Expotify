import { useState, useRef, useEffect, useLayoutEffect, type KeyboardEvent } from "react";
import Markdown from "react-markdown";
import { RotateCcw } from "lucide-react";
import type { ChatEntry } from "../hooks/useAgentChat";
import { useIMEComposition } from "../hooks/useIMEComposition";

interface AgentChatProps {
  onClose: () => void;
  entries: ChatEntry[];
  loading: boolean;
  sendMessage: (text: string) => void;
  reset: () => void;
  cancel: () => void;
  chatReadEnabled: boolean;
  onToggleChatRead: () => void;
  ttsVolume: number;
  onTtsVolumeChange: (vol: number) => void;
}

export function AgentChat({
  onClose,
  entries,
  loading,
  sendMessage,
  reset,
  cancel,
  chatReadEnabled,
  onToggleChatRead,
  ttsVolume,
  onTtsVolumeChange,
}: AgentChatProps) {
  const [input, setInput] = useState("");
  const messagesRef = useRef<HTMLDivElement>(null);
  const followLatestRef = useRef(true);
  const inputRef = useRef<HTMLInputElement>(null);
  const { onCompositionEnd, isIMEEnter } = useIMEComposition();

  useLayoutEffect(() => {
    const messages = messagesRef.current;
    if (messages && followLatestRef.current) {
      // Scroll only the transcript, never its clipped overlay ancestors.
      messages.scrollTop = messages.scrollHeight;
    }
  }, [entries, loading]);

  useEffect(() => {
    inputRef.current?.focus({ preventScroll: true });
  }, []);

  const handleSend = () => {
    const text = input.trim();
    if (!text || loading) return;
    followLatestRef.current = true;
    setInput("");
    sendMessage(text);
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey && !isIMEEnter()) {
      e.preventDefault();
      handleSend();
    }
    if (e.key === "Escape") {
      if (loading) {
        cancel();
      } else {
        onClose();
      }
    }
  };

  return (
    <div className="agent-chat" data-no-drag="true">
      <div className="agent-chat-header">
        <span className="agent-chat-title">Chat</span>
        <div className="agent-chat-header-btns">
          <button
            className={`agent-chat-read-toggle${chatReadEnabled ? " active" : ""}`}
            onClick={onToggleChatRead}
            aria-pressed={chatReadEnabled}
            title={chatReadEnabled ? "Automatically read chat replies: ON" : "Automatically read chat replies: OFF"}
          >
            <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M11 5L5.5 7.5V12.5L11 10Z" />
              <path d="M11 5L16.5 7.5V12.5L11 10Z" />
              <circle cx="4" cy="3.5" r="2" />
              <path d="M2 6.5V13" />
            </svg>
            Auto Read
          </button>
          <div className="overlay-tts-volume" data-no-drag="true">
            <svg width="8" height="8" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 2.5L4.5 5.5H2v5h2.5L8 13.5V2.5z" />
              {ttsVolume > 0 && <path d="M10.5 5.5a3.5 3.5 0 010 5" fill="none" stroke="currentColor" strokeWidth="1.2" />}
            </svg>
            <input
              type="range"
              className="overlay-tts-slider"
              min={0}
              max={100}
              aria-label="Speech volume"
              value={Math.round(ttsVolume * 100)}
              onChange={(e) => onTtsVolumeChange(Number(e.target.value) / 100)}
            />
          </div>
          <button className="agent-chat-reset" onClick={reset} title="Reset conversation" aria-label="Reset conversation">
            <RotateCcw size={12} />
          </button>
        </div>
      </div>
      <div
        ref={messagesRef}
        className="agent-chat-messages"
        onScroll={(event) => {
          const messages = event.currentTarget;
          followLatestRef.current = messages.scrollHeight - messages.clientHeight - messages.scrollTop < 40;
        }}
      >
        {entries.length === 0 && (
          <div className="agent-chat-empty">Ask me to search and play music, like songs, or adjust volume</div>
        )}
        {entries.map((entry) => (
          <div key={entry.id} className={`agent-chat-msg ${entry.role}`}>
            {entry.role === "user" && <span className="agent-chat-label">You</span>}
            {entry.role === "assistant" && entry.action && entry.action !== "reply" && entry.action !== "ask" && entry.action !== "refuse" && (
              <span className="agent-chat-action">{entry.action}</span>
            )}
            <span className="agent-chat-text">{entry.role === "assistant" ? <Markdown>{entry.content}</Markdown> : entry.content}</span>
          </div>
        ))}
        {loading && (
          <div className="agent-chat-msg system">
            <span className="agent-chat-text agent-chat-loading-dots">Thinking</span>
          </div>
        )}
      </div>
      <div className="agent-chat-input-row">
        <input
          ref={inputRef}
          className="agent-chat-input"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          onCompositionEnd={onCompositionEnd}
          placeholder="Type a message..."
          disabled={loading}
        />
        {loading ? (
          <button
            className="agent-chat-stop"
            onClick={cancel}
            title="Stop (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
              <rect x="3" y="3" width="10" height="10" rx="1" />
            </svg>
          </button>
        ) : (
          <button
            className="agent-chat-send"
            onClick={handleSend}
            disabled={!input.trim()}
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
              <path d="M2 14l12-6L2 2v5l8 1-8 1v5z" />
            </svg>
          </button>
        )}
      </div>
    </div>
  );
}
