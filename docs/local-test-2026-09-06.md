# Local Model Runtime Test: 2026-09-06

Scope: macOS arm64 debug bundle, native Computer Use against the packaged app. The installed release was stopped by the user and was not replaced. Settings were backed up before launch; Claude browser authorization was completed by the user.

## Verified in the App

- The main window identifies itself as `Expotify (Local Test)`.
- The 360 x 480 model picker keeps its header/footer visible while the model list scrolls. Home, End, arrow selection, Escape, and return-to-trigger focus worked.
- With only ChatGPT connected, only its Default and concrete catalog were offered. The disconnected legacy Claude selection stayed visible instead of being silently changed.
- Default and a concrete model matching Default were saved as distinct selections. Prompt templates and memories were unchanged after saving.
- Explicit catalog refresh updated the freshness timestamp.
- Claude login entered its waiting state, cancellation restored Connect, and no login subprocess remained after cancellation. Retry and the browser callback succeeded.
- With both accounts connected, the picker showed two Defaults and both catalogs (12 options in this account). Observed defaults were ChatGPT `GPT-6-Astra` and Claude `Opus 5 (1M)`; these are observations, not hardcoded product defaults.
- The Claude catalog also included fixed `Opus 5 (1M)`, `Fable 5.1`, `Sonnet 5`, and `Haiku 4.5` entries.
- Claude Default generated a complete Insight through the bundled runtime.
- Fixed Fable 5.1 returned `FABLE_OK` in Chat.

Chat automatic reading was temporarily disabled to avoid interrupting the user's video. No playback, volume, or memory-writing tool was intentionally requested in test messages.

## Defect Found and Corrected

ChatGPT Default returned a short acknowledgement three times. The parser appended deltas, `output_text.done`, and `content_part.done` snapshots to the same string. A regression test reproduced `readyreadyready` before the fix.

The parser now accumulates by output/content index, replaces each part with its complete snapshot, and prefers a completed response when present. Failed/incomplete responses and malformed events are errors, not executable partial chat results. Framing tests cover CRLF, multiline data, and `data:` without a space. This follows the typed-event distinction in the [OpenAI streaming documentation](https://developers.openai.com/api/docs/guides/streaming-responses).

Independent review passed the parser change and the test-only timeout adjustment. All 34 Rust tests and 12 frontend/helper tests passed. The rebuilt package-local runtime smoke also passed without external Node/Bun/Claude on PATH.

## Final Checks

- Passed: rebuilt debug bundle and package-local runtime smoke.
- Passed: the identical ChatGPT Default test message produces its acknowledgement once in the rebuilt app.
- Passed: restart preserves both connections, Claude Default for Insight, and fixed Fable for Chat. Chat was then set back to ChatGPT Default for the regression check.
- Passed: final disk comparison confirms the original prompt templates and memories are unchanged.
- Pending user confirmation: clear the two test-only chat entries before restoring Chat Auto Read, so enabling it does not immediately speak the last test reply.

## Follow-Up Observations

- On the first attempt to open the restarted main window, Computer Use saw a blank window and reported `windowNotFoundAtPosition`. Reopening from the overlay restored the full window and normal interaction. This was not diagnosed as a code defect; retain it as a startup/window-activation observation.
- Unverified reviewer hypothesis: the debug-only `main.show()` in `src-tauri/src/lib.rs` may run before the WebView's first paint and overlap with the overlay's Open flow. That branch is debug-only, but the observed blank window has not been causally attributed to it. No window behavior was changed.
- Existing Chat Auto Read behavior needs separate attention: disabling it invokes `skipChatTts`, which unconditionally resumes Spotify even when nothing was being read; enabling it processes the current last assistant message. These paths in `src/overlay/OverlayApp.tsx` were not changed in the model/runtime work. No TTS quality claim is made by this test.

## Limits

This is not a public release, notarization check, exhaustive test of every model, or a full network/quota failure test. Concurrent long Claude Insight plus Claude Chat still needs a dedicated UI test; backend concurrency/auth-status guards have unit coverage. No claim is made about music factual accuracy, TTS audio quality, or unrelated playback controls.

Returning to the old installed app requires stopping the test app and restoring the pre-migration settings backup first; see [runtime notes](model-runtime.md).
