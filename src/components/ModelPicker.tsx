import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, CircleAlert, RefreshCw, X } from "lucide-react";
import type { AuthStatus, ModelProvider, ModelSelection, ProviderCatalog } from "../types";
import { describeSelection, modelOptions, providerLabel, sameSelection, selectionKey } from "../lib/models";
import openaiIcon from "../assets/providers/openai.svg";
import claudeIcon from "../assets/providers/claude.svg";
import "./ModelPicker.css";

function ProviderIcon({ provider }: { provider: ModelProvider }) {
  return <img className={`model-brand ${provider}`} src={provider === "openai" ? openaiIcon : claudeIcon} alt="" />;
}

interface Props {
  value: ModelSelection;
  onChange: (selection: ModelSelection) => void;
  catalogs: ProviderCatalog[];
  auth: AuthStatus;
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
}

export function ModelPicker({ value, onChange, catalogs, auth, loading, error, onRefresh }: Props) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({});
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const options = useMemo(() => modelOptions(catalogs, auth), [catalogs, auth.openai, auth.anthropic]);
  const selected = describeSelection(value, catalogs, auth);
  const connected = auth.openai || auth.anthropic;
  const errors = catalogs.filter(catalog => catalog.error);
  const stale = catalogs.some(catalog => catalog.stale);
  const updated = catalogs.map(catalog => catalog.fetched_at).filter((date): date is string => !!date).sort()[0];

  const close = (restore = true) => {
    setOpen(false);
    if (restore) trigger.current?.focus();
  };

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      const available = window.innerHeight - rect.bottom - 16;
      if (available < 260 || window.innerWidth < 400) {
        setPosition({ left: 12, right: 12, top: 12, maxHeight: "calc(100dvh - 24px)", width: "auto" });
      } else {
        const width = Math.min(380, window.innerWidth - 24);
        setPosition({ left: Math.max(12, Math.min(rect.left, window.innerWidth - width - 12)), top: rect.bottom + 6,
          width, maxHeight: Math.min(420, available) });
      }
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => { window.removeEventListener("resize", place); window.removeEventListener("scroll", place, true); };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    (panel.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]:not(:disabled)')
      ?? panel.current?.querySelector<HTMLButtonElement>('[role="option"]:not(:disabled)')
      ?? panel.current?.querySelector<HTMLButtonElement>('[aria-label="Close model list"]'))?.focus();
    const outside = (event: PointerEvent) => {
      const node = event.target as Node;
      if (!panel.current?.contains(node) && !trigger.current?.contains(node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (event.key === "Tab") { close(); return; }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const buttons = Array.from(panel.current?.querySelectorAll<HTMLButtonElement>('[role="option"]:not(:disabled)') ?? []);
    if (!buttons.length) return;
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
      : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
    buttons[next].focus();
    buttons[next].scrollIntoView({ block: "nearest" });
  };

  return <div className="model-picker">
    <div className="model-label-row">
      <label className="field-label" id={`${id}-label`}>Model</label>
      <button type="button" className="model-icon-button" aria-label="Refresh models" title="Refresh models"
        disabled={!connected || loading} onClick={onRefresh}>
        <RefreshCw size={14} className={loading ? "model-spin" : undefined} />
      </button>
    </div>
    <button ref={trigger} type="button" className={`model-trigger${selected.disabled ? " unavailable" : ""}`}
      aria-labelledby={`${id}-label ${id}-value`} aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? `${id}-list` : undefined}
      onClick={() => setOpen(!open)} onKeyDown={event => {
        if (["ArrowDown", "ArrowUp"].includes(event.key)) { event.preventDefault(); setOpen(true); }
      }}>
      <ProviderIcon provider={value.provider} />
      <span className="model-copy" id={`${id}-value`}>
        <span className="model-title"><span className="model-provider">{providerLabel(value.provider)}</span><span>{selected.name}</span></span>
        {selected.detail && <span className="model-detail">{selected.detail}</span>}
      </span>
      <ChevronDown size={16} className="model-chevron" />
    </button>
    <div className={`model-status${stale || error || selected.disabled ? " warning" : ""}`} role="status">
      {loading ? "Refreshing models..." : !connected ? "No accounts connected" : error ? "Model refresh failed"
        : stale ? "Using last available model list" : selected.disabled ? "Selected model unavailable"
        : updated ? `Updated ${new Date(updated).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : "Model list unavailable"}
    </div>
    {(error || errors.length > 0) && <details className="model-errors">
      <summary><CircleAlert size={12} /> Refresh details</summary>
      {error && <p>{error}</p>}
      {errors.map(catalog => <p key={catalog.provider}>{providerLabel(catalog.provider)}: {catalog.error}</p>)}
    </details>}
    {open && createPortal(<div ref={panel} className="model-panel" style={position} onKeyDown={onKeyDown} onClick={event => event.stopPropagation()}>
      <div className="model-panel-header">
        <span>Models</span>
        <div className="model-panel-tools">
          <button type="button" className="model-icon-button" aria-label="Refresh model list" title="Refresh models" disabled={!connected || loading} onClick={onRefresh}>
            <RefreshCw size={15} className={loading ? "model-spin" : undefined} />
          </button>
          <button type="button" className="model-icon-button" aria-label="Close model list" title="Close" onClick={() => close()}><X size={17} /></button>
        </div>
      </div>
      <div className="model-list" id={`${id}-list`} role="listbox" aria-label="Models" aria-busy={loading}>
        {!connected && <div className="model-empty">No accounts connected</div>}
        {options.map((option, index) => {
          const previous = options[index - 1];
          const heading = option.selection.mode === "default" ? (index === 0 ? "Defaults" : null)
            : previous?.selection.mode === "default" || previous?.selection.provider !== option.selection.provider ? providerLabel(option.selection.provider) : null;
          return <div key={selectionKey(option.selection)} role="presentation">
            {heading && <div className="model-group" role="presentation">{heading}</div>}
            <button type="button" role="option" className="model-option" aria-selected={sameSelection(value, option.selection)}
              disabled={option.disabled} tabIndex={-1} title={option.selection.mode === "fixed" ? option.selection.model : option.detail}
              onClick={() => { onChange(option.selection); close(); }}>
              <ProviderIcon provider={option.selection.provider} />
              <span className="model-copy">
                <span className="model-title"><span className="model-provider">{providerLabel(option.selection.provider)}</span><span>{option.name}</span></span>
                {option.detail && <span className="model-detail">{option.detail}</span>}
              </span>
              <span className="model-check">{sameSelection(value, option.selection) && <Check size={16} />}</span>
            </button>
          </div>;
        })}
      </div>
      <div className="model-panel-status" role="status">{loading ? "Refreshing models..." : stale || error ? "Last refresh failed" : `${options.length} options`}</div>
    </div>, trigger.current?.closest('[role="dialog"]') ?? document.body)}
  </div>;
}
