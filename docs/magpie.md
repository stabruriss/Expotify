# Magpie Gateway

[Magpie](https://github.com/yetone/magpie) is a gateway the user installs, signs in to and runs on their own Mac. Expotify offers its models as a third provider next to ChatGPT and Claude. Expotify never starts Magpie, never reads its accounts or keys, and stores no credential for it. Only the local gateway on `127.0.0.1` is supported; there is no LAN or remote mode and no manual port setting.

Every request carries `Authorization: Bearer magpie-expotify`. On loopback Magpie checks no key: it reads `magpie-<app>` as the caller's name, which it uses for usage attribution and for per-app model visibility and order. It is a name, not a secret, and it is never another app's name.

Requests go straight from Rust to `http://127.0.0.1:<port>`, without the system proxy and without following redirects. They never come from the webview, so they carry no browser headers that Magpie would treat as DNS rebinding.

## Port

The port is read again for every discovery and every request, because Magpie can move to another port without a restart. Expotify reads only the `port` field of Magpie's settings file, `$XDG_CONFIG_HOME/magpie/settings.json` when that variable is set, otherwise `~/.config/magpie/settings.json` (also on macOS). A missing file or a port outside 1024–65535 means 3425, as in Magpie itself. The file is never written, cached or logged.

Limits: an app opened from Finder does not see a shell's `XDG_CONFIG_HOME`, and a Magpie started with `MAGPIE_ADDR` or run as a portable install keeps its address where Expotify cannot see it. Expotify then looks only at the port in `~/.config/magpie/settings.json`, or 3425. It never scans other ports.

## Discovery

A discovery is `GET /api/hello` followed by `GET /v1/models`, within one 10-second budget. Magpie answers hello itself, without an upstream or quota, with `{"name":"magpie",…}`. Any other answer, 401/403 included, means the port belongs to something else, and the model list is not requested.

Discovery runs only on explicit actions: opening Settings or the model picker, Refresh, and once at launch when the saved Insight or Chat choice is Magpie. It never runs from the overlay's status poll, window focus or a timer. Calls that arrive while a discovery runs share its result. For Magpie, `get_auth_status` and `list_models` only read the last discovery and never contact the gateway; `list_models` still refreshes the ChatGPT and Claude catalogs as before.

| Last discovery | Model list | `magpie` status |
|---|---|---|
| Nothing listens, no hello within the budget, or not Magpie | No Magpie group and no error | false |
| Magpie answered hello, but the model list failed (HTTP error, unreadable, stalled or cut off) | The last successful list, marked stale, with the reason in Refresh details | false |
| Success | Magpie's list | true when it has at least one fixed model |

An empty list (`{"object":"list","data":[]}`) is a successful discovery with no models.

## Catalog

Models keep Magpie's order and their ids verbatim (`provider/model`, for example `codex/gpt-5.5`). The name is Magpie's `display_name`, or the id when it is missing or blank. Nothing is prefixed, and models with the same display name from different providers stay separate entries. A repeated id keeps its first entry.

These are left out because they are not a fixed model, or cannot be saved:

- `group/…` route groups, bare ids without `/` (Magpie routes them through a group) and `copilot/auto` (Copilot picks a model per request).
- Ids that Settings would reject: longer than 200 bytes, or containing control characters.

Magpie has no default model, so there is no Magpie Default: the catalog's `default_model` is always null, and only fixed models are offered. A saved Magpie choice is kept when the gateway is offline or the model has left the list; it is shown as unavailable and never replaced. Saving Settings checks only the format, so a saved choice that has become unusable never blocks saving other settings.

## Generation

Insight and Chat use `POST /v1/chat/completions` with `stream: false`. Each request first reads the port and checks hello; every round of that request then goes to the same address. Each round sends the whole conversation, without `previous_response_id`, and the shared tool registry as function tools without `strict`. Tool results go back as `role: "tool"` messages with the model's call id. Magpie has native tool calling only; the hidden legacy protocol setting does not apply to it.

- A saved Default, route group, bare id or `copilot/auto` is refused when a request starts, with a prompt to select a fixed Magpie model.
- The cached model list does not gate requests. Magpie's answer decides, for example 404 for a model it does not know or 403 for a model turned off in Magpie.
- An HTTP error is reported with Magpie's own reason, bounded to 600 characters, and never with a raw or HTML body. `Retry-After` is shown as a hint. Nothing is retried automatically, and nothing switches to another provider.
- Each generation HTTP request (the Insight request, or one chat round) may take up to 240 seconds. That comes on top of the 10-second hello and any tool execution between rounds; it is not a limit for a whole chat. In Chat, the hello, each send and each reply read can be cancelled; Insight has no cancel action.

### Which model answered

Magpie names the model that produced a non-streaming reply in the `X-Magpie-Model` header. Every successful reply, for Insight and for each chat round, is used only when that header appears exactly once and equals the requested id byte for byte. Otherwise the reply is discarded before its body is read: no tool call in it runs, and Insight shows the error instead of the text. Tool results from earlier rounds of the same chat request are kept and reported, and nothing is replayed. The `model` field in the body is ignored, because Magpie may put the vendor's name or, with its MemberModel setting, the full id there. HTTP errors are reported first; the header check applies only to successful replies.

What this does and does not guarantee: a provider's fallback list in Magpie can move a request to another model before the first byte, and Magpie has no per-request switch to turn that off. Expotify rejects the substituted reply and every action it asks for. It cannot stop Magpie from having sent the conversation to the fallback model, or that model's quota from being used. The header names only the `provider/model` Magpie sent the request to; routing inside a vendor is not visible. The check depends on non-streaming replies (when streaming, Magpie can send the header before a fallback takes over), which is why Expotify does not stream from Magpie.

### Rounds and tool calls

- A reply is used only when `finish_reason` is `stop` or `tool_calls`. Magpie reports `length` or `content_filter` for an incomplete reply; such a reply, and one without a finish reason, is rejected.
- All tool calls of a round are checked before any of them runs. Each needs an id and a function name, both non-empty and without control characters, type `function`, and ids unique within the round. One malformed call rejects the whole round, and nothing in it runs.
- Arguments: an empty string means no arguments, and a JSON object goes to the executor's validator. `null`, arrays, scalars, invalid JSON, and missing or non-string arguments come back to the model as an `invalid_argument` failure; they never run as "no arguments".
- The executor is the one ChatGPT and Claude use: each call id runs at most once, results are what actually happened, there are at most three rounds, and plain text never becomes an action.
- The assistant message goes back whole with the tool results, including reasoning fields such as `reasoning_content`, which Magpie hands to upstreams that need them. Only its text is shown.

### Cancellation

Cancelling a chat drops the HTTP request. The fake-gateway tests verify that the connection closes and that no tool runs after the cancel; a tool that is already running finishes and is reported. That Magpie then stops its own upstream request (a Claude subscription turn is rewound or ended) comes from Magpie's source and documentation, and has not been tested against a real gateway. Tokens the vendor already processed still count, and quota already used is not refunded.

### Web search

With Web Search on, the request includes `web_search_options: {}`. Magpie decides how to search: the upstream's own search, a searcher model, or a search API. With no search means configured, it answers without searching and without saying so. A Chat Completions reply carries no record of a search, so Expotify neither reports that the model searched nor says that search is unsupported. `ai_used_web_search` stays false for Magpie, which is not proof that no search happened.

## Debug probe

In debug builds, `EXPOTIFY_TOOL_PROBE=magpie` (or `tool_protocol_probe` with `magpie`) runs one dry-run tool round against the Magpie model selected for Chat. Magpie has no Default to probe. The probe is tested only against the fake gateway. A real run sends a real request and uses the account's quota, so run it only with explicit approval.

## Tests and compatibility

`cargo test --lib` covers the gateway with a loopback fake gateway, in `ai::magpie` tests. The cases cover discovery, absence, stale lists, the port file, the served-model check, round and argument checks, cancellation, HTTP errors and Insight; none of them contacts a real Magpie.

The installed Expotify 0.5.9 does not recognise a Magpie choice in `settings.json` and fails to start. Back up `settings.json` before saving a Magpie choice for the first time. To go back to 0.5.9, either switch Insight and Chat to ChatGPT or Claude and then quit this build, or quit this build and then restore the backup. Never start 0.5.9 while this build is running.
