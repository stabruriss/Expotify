import { useRef, useCallback } from "react";

/**
 * Hook to prevent IME composition Enter from triggering send.
 *
 * WebKit (Safari / Tauri WKWebView) fires compositionend BEFORE keydown,
 * so `e.isComposing` is already false when the Enter keydown arrives.
 * We track the compositionend timestamp and ignore Enter within a short window.
 */
export function useIMEComposition(thresholdMs = 100) {
  const compositionEndTime = useRef(0);

  const onCompositionEnd = useCallback(() => {
    compositionEndTime.current = Date.now();
  }, []);

  const isIMEEnter = useCallback(() => {
    return Date.now() - compositionEndTime.current < thresholdMs;
  }, [thresholdMs]);

  return { onCompositionEnd, isIMEEnter };
}
