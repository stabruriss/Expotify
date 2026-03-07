import { useState, useRef, useEffect, type KeyboardEvent } from "react";
import Markdown from "react-markdown";
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
  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const { onCompositionEnd, isIMEEnter } = useIMEComposition();

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [entries]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleSend = () => {
    const text = input.trim();
    if (!text || loading) return;
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
            title={chatReadEnabled ? "Auto read: ON" : "Auto read: OFF"}
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
              value={Math.round(ttsVolume * 100)}
              onChange={(e) => onTtsVolumeChange(Number(e.target.value) / 100)}
            />
          </div>
          <button className="agent-chat-reset" onClick={reset} title="Reset conversation">
            <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M2 8a6 6 0 0110.47-4" />
              <path d="M14 8a6 6 0 01-10.47 4" />
              <path d="M12.47 1v3h-3" />
              <path d="M3.53 15v-3h3" />
            </svg>
            Reset
          </button>
        </div>
      </div>
      <div className="agent-chat-messages">
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
        <div ref={bottomRef} />
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
