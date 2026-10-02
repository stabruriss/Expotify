# Chat Tool Calling

How the chat assistant's actions (play, like, volume, memory, prompt updates) are requested
by the model and executed by the app.

## One executor, two protocols

All tool execution happens in Rust (`src-tauri/src/ai/tools.rs`):

- `definitions()` is the single registry: name, description and a strict JSON Schema
  (every property required, `additionalProperties: false`) for each of the seven tools.
- `validate()` turns a `ToolCall { id, name, args }` into a typed invocation. Numeric
  arguments must be JSON numbers (an integral float is accepted, a numeric string is not);
  enums and ranges are enforced; unknown tool names are errors.
- `ToolRunner::run()` executes at most once per call id, refuses to run after the request
  was cancelled, and always returns a `ToolOutcome { call_id, ok, output, error_code, track_name }`.
  Every failure branch produces an explicit outcome; nothing is reported as done unless
  the executor did it. `output` is the text both the model and the UI read. The outcome is
  recorded (and its `exec` event written) the moment the call finishes, so it survives a
  provider failure or a cancellation later in the same request.

The protocol only decides how a call reaches the executor. It is chosen per provider in
`Settings.tool_protocol` **before** the request starts and is never switched mid-request.
Both providers default to `native`; `legacy` has no UI entry and exists for diagnosis
(`"tool_protocol": {"openai": "legacy"}` in `settings.json`). The shipped chat prompt no
longer contains the JSON convention; a stored prompt that still equals one of the old
defaults is migrated on load, a customised prompt is adapted per request
(`native_system_prompt` / `legacy_system_prompt`).

| Protocol | Claude | ChatGPT |
| --- | --- | --- |
| `legacy` (diagnosis only, no UI entry) | Prompt asks for a JSON object; `parse_agent_response` extracts it from the text; at most one action per reply. | Same. |
| `native` (default) | Agent SDK in-process MCP tools (`createSdkMcpServer`) registered from the Rust registry's schema; each `tool_use` is relayed to Rust over the helper's stdin/stdout line protocol and answered before the model continues; up to 4 model turns. | Responses `tools` (strict function definitions + `web_search`), `tool_choice: auto`, `parallel_tool_calls: false`; `function_call` items are executed and answered with `function_call_output`, carrying the turn's output items (including reasoning) back verbatim; up to 3 rounds. |

In native mode plain assistant text is **never** interpreted as an action, even if it looks
like JSON. The legacy parser exists only on the legacy path.

## Helper line protocol (Claude native)

Rust writes the request as one JSON line and keeps stdin open. The helper writes
`{"type":"tool_call","id","name","args"}` lines to stdout and waits for
`{"type":"tool_result","id","ok","output"}` on stdin; the final line is the usual
`{"ok":true,"data":{text,turns,tool_uses}}`. `claude_runtime::ClaudeSession` drives this
(`start` → `next` → `reply`); dropping the session kills the process group. The helper
holds no Spotify credentials or execution logic.

## Cancellation, deduplication, serialization

- Every `agent_chat` call carries a client-chosen `request_id`; `agent_chat_cancel(request_id)`
  only acts on that request, so a cancel that arrives late (after a newer request was
  registered) is ignored instead of aborting the newer one.
- Cancellation is cooperative. It aborts the wait for the provider (the HTTP request
  is dropped, the helper process group is killed) and sets the runner's cancelled flag so no
  further tool executes. A tool that is already running is never interrupted: it finishes,
  its outcome is recorded, and the request then returns. Actions already performed stay
  done; nothing is replayed or rolled back, and the request never falls back to another
  protocol after a partial execution. Starting a new chat request cancels the previous one
  the same way.
- A request that ends early (provider error or cancellation) after at least one tool ran
  still returns `AgentChatResult` with those `tool_results`, an empty reply and `error` set
  (`"Cancelled"` for a cancellation). Only a request with no executed tool fails with a
  plain error.
- Call ids are executed once; a repeated id returns the recorded outcome.
- Tool calls within a request run sequentially; settings writes are serialized by the
  settings lock across requests.

## Choosing the track and updating prompts

`search_and_play` takes `query` (search text, including the artist when one was named) and
`artist` (the artist the user named, or empty). With a named artist the executor fetches the
top ten results and plays the first one credited to exactly that artist (names compared
case- and whitespace-insensitively; substrings, short forms, translations and tribute acts do
not count), so a popular cover that Spotify ranks first for this account does not win over the
requested version; when none of the ten matches nothing is played and the call fails with
`artist_not_found`, listing the closest matches so the model can ask the user instead of
accepting a substitute on their behalf (the request's track focus becomes unconfirmed, as
after any failed playback change).
`update_prompt` takes `mode`: `replace` overwrites the prompt, `append` keeps the current
prompt and adds the content after it (the model cannot read the current prompt, so
additions must go through `append`). Both arguments are optional for the legacy text
protocol (empty artist, replace).

### Playback confirmation

`spotify_play_track` runs the play command as its own AppleScript step; hiding the Spotify
window and restoring focus afterwards is a separate, best-effort step whose failure is only
logged. When the play command still reports an error, the executor reads the player's current
track: if it is the requested track the outcome is a success that mentions the error;
otherwise the outcome is `play_failed` and says that playback may have started and the
current song is unknown, so the model asks instead of asserting the previous song is still
playing (the request's track focus is unconfirmed either way until a success).

## "The current song" inside one request

`ToolContext` starts with the track that was playing when the request began and follows the
executor's own confirmed playback changes (`TrackFocus`): after a successful
`search_and_play` / `shuffle_liked`, `like_current` / `unlike_current` act on the track just
played, so "play X, then like it" likes X without waiting for the UI's track polling. After a
failed playback change in the same request the focus is `Unconfirmed` and like/unlike fail
with `track_unconfirmed` instead of touching whichever song happened to be playing.

## Result aggregation

`AgentChatResult.executed` is true only when at least one tool ran and every call succeeded;
`track_name` is the track now playing because of this request (the last successful play
call, not the last call); `error` is a request-level failure, never a single tool's failure.
The frontend store (`src/lib/chatStore.ts`, wrapped by `useAgentChat`) walks `tool_results`
one by one: every failure becomes an `Action failed` status line, every track change a
`Now playing` line, like/unlike and memory refreshes fire per outcome, and when the request
ended without a reply every success is listed too. Each request has its own id: a cancelled
or superseded request may only report what the executor already did, never add a reply or
clear the newer request's loading state; its assistant turn is inserted right after its own
user message (located by identity, not by index). `reset` starts a new conversation
generation and cancels the in-flight request, which can then no longer write into the new
session. `src/lib/chatStore.test.ts` covers these cases without React or Tauri.

## History and UI

`AgentChatResult.tool_results` lists every outcome in order. The frontend keeps them on the
assistant history message (`ChatMessage.tool_results`) so later turns see what actually
happened, and tags entries with `kind` (`user`, `final`, `tool_status`, `notice`). Only
`final` entries are meant to be read aloud.

## Event log

`src-tauri/src/ai/events.rs` appends one JSON line per event to `tool-events.jsonl` next
to `settings.json` (rotated at 2 MB): `request_id` (random, content-free, shared by every
event of one request), provider, model, protocol, stage, tool name, `call_id` (matches
`tool_results[].call_id`), ok, error code, legacy parse path, turns, duration. No
conversation text, prompts, queries or arguments are logged.

- `exec` is written by the runner as soon as a call finishes, before the model continues.
- `turn` is written for every request that reached a provider, including failed and
  cancelled ones: `ok` is true only when the request completed and every tool succeeded;
  `error_code` is `cancelled`, `provider_error` or `tool_failed`. Requests that fail before a
  provider is reached (model resolution, provider not connected) write no events.
- `parse` exists only on the legacy path.

Writing is best effort (a write failure is logged and ignored, and a process that exits
mid-call writes nothing), so the log is for cross-checking when the app ran normally; the
executor's `tool_results` and the real Spotify/settings state remain the source of truth.

## Endpoint probe (debug builds only)

Before enabling a native protocol, verify the provider end to end with a dry-run executor
(calls are validated and reported, never executed):

- Tauri command `tool_protocol_probe(provider: "openai" | "anthropic")`, or
- start a debug build with `EXPOTIFY_TOOL_PROBE=openai` (or `anthropic`). The app waits
  5 s after startup, moves an earlier `tool-probe-<provider>.json` aside as
  `tool-probe-<provider>.prev.json`, runs the probe (model round trips take seconds to tens
  of seconds) and only then writes the new report next to `settings.json`. Poll for the file
  instead of sleeping a fixed time.

The report carries `run_id`, `started_at`/`finished_at`, provider, model, `tool_calls`
(`call_id`, `name`, `ok`, validated `args`, `output`), turns, `text`, `error` and a `pass`
verdict computed from `pass_criteria`: ok, exactly one tool call, it is `set_volume` with
`ok=true` and numeric `level` 42, non-empty final text. A failure before the model was
reached (model resolution, provider not connected) still produces a report with the run
metadata, `ok: false` and `error`. Arguments appear in this debug report only, never in the
event log.

The probe uses the app's own connected accounts. Quit the production app first so two
processes never refresh the same credential. Release builds reject the command and do not
compile the startup trigger.

## Fault injection (debug builds only)

`EXPOTIFY_FAULTS=name1,name2` (`src-tauri/src/faults.rs`) activates named faults for one
process; nothing is persisted and release builds ignore the variable. Executor faults:
`spotify_not_connected` makes every Spotify Web API action fail with that error code without
touching credentials; `tool_exec_fail` fails every validated call with `injected_failure`
before it runs; `tool_delay` holds the request for 5 s after each executed action so a tester
can cancel between the actions of one request. Other modules define their own names through
the same `faults::active` (the speech module uses `tts_*` names).
