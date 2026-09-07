# Model Selection and Bundled Claude Runtime

## Selection Contract

Insight and Chat each persist either `{ provider, mode: "default" }` or `{ provider, mode: "fixed", model }`. Default resolves from the latest successful catalog at request time. A fixed choice keeps its concrete wire ID, even when it is also today's default. No provider or model substitution occurs on errors.

New installs use the first connected account's Default. Legacy model strings migrate to fixed choices without changing provider or model. Unavailable old choices remain visible until explicitly changed. Only connected providers appear in the picker.

Before loading a legacy string selection, Expotify creates a one-time private `settings.json.pre-model-selection.bak` beside `settings.json`. Loading or backing up invalid/unreadable settings fails startup instead of replacing user data with factory defaults. The new format is forward-only: **do not run the old and new apps concurrently** against the same settings and ChatGPT credentials. Stop the old app before testing; the installed `.app` itself need not be replaced.

Catalogs are single-flight, cached for five minutes in the backend, refreshed on opening Settings or explicitly with Refresh. Failed refreshes preserve the last successful snapshot, visibly marked stale. Disconnecting discards the provider service and its catalog. A cold failure does not fabricate a default.

## Sources

- ChatGPT uses the existing authenticated Codex HTTP model catalog and Responses endpoint, not a local CLI. The catalog's visible, non-retired models are sorted by server priority; the leading model supplies Default. This is a compatibility endpoint, not the public API-key Models API. A protocol change must fail visibly rather than substitute a hardcoded ID.
- Claude uses Agent SDK `supportedModels()` and each row's `resolvedModel`. The `default` row is resolved by the native runtime with its own authenticated account metadata and managed policies. The account gate uses `auth status --json` raw `authMethod`, `apiProvider`, and `subscriptionType`, not SDK display labels. This is the **provider runtime default**, not a preference copied from an independently installed official client.
- The pinned Claude CLI updates its bootstrap cache in the background; the helper waits a bounded interval and takes a second SDK initialization snapshot. Bootstrap timestamps and freshness are reported; no prompt or unknown-model validation request is sent while listing models.

Fresh means bootstrap succeeded and either changed data was persisted or the provider confirmed the cached data was unchanged. The pinned CLI's unchanged-cache diagnostic is checked at build time and observed only through an in-memory stderr callback. Unknown/failed freshness remains visibly stale.

## Authentication Boundary

Claude authentication and refresh are exclusively owned by the bundled native CLI, using `claude auth login --claudeai`. Expotify supplies an isolated `CLAUDE_CONFIG_DIR` under its app-data directory. Native CLI `auth status --json` verifies connection without generating tokens.

The legacy App-owned Claude refresh token is not imported, deleted or rotated. Existing users reconnect Claude once. Global Claude settings/credentials are not modified. Inherited API keys, OAuth tokens, model overrides and alternative-provider variables are removed. SDK user/project settings and tools are disabled; its working directory is private and empty. The native CLI is never auto-updated in place.

Before catalog discovery and each generation request, the helper verifies the native credential source is a first-party `claude.ai` subscription; it refuses Console/API-key/profile fallthrough. Authentication failures invalidate the App's local status cache. Runtime `model_refusal_fallback` events stop the request with an explicit error instead of returning a result from a substituted model. Browser callback failure has a bounded login deadline and an actionable retry message; manual authorization-code entry is not supported.

Subprocesses have operation deadlines, bounded stdout, cancellation and process-group cleanup. Claude calls are serialized to keep one credential-refresh owner. ChatGPT token operations are serialized independently. Native auth status is initialized once per App process and changed on login/logout; the overlay's ten-second local status poll does not spawn subprocesses or wait behind model generation. Token validity is still checked by the provider when a request is made.

Anthropic documents a [prior-approval requirement for third-party subscription login](https://code.claude.com/docs/en/agent-sdk/overview). Engineering validation and signing/notarization do not establish that approval; release owners must evaluate distribution against the provider's terms.

## Building and Testing

Build machines need Node (frontend/scripts), Rust/Tauri, and Bun 1.3.5. `npm ci` installs the exact SDK/native package. `npm run build:claude` compiles the helper and stages a sibling native `claude` binary; Tauri includes both as resources. End users on macOS 13+ need none of those development runtimes or an external Claude/Codex installation. Build on a matching arm64/x64 host; cross/universal builds are rejected until a multi-architecture runtime is provided.

Release CI signs both runtimes with hardened runtime/JIT entitlements before signing and notarizing the outer artifact. See [Bun signing guidance](https://bun.sh/guides/runtime/codesign-macos-executable). No signing/notarization is implied by a debug build.

Checks: `npm test`, `npm run build`, `cargo test --lib` inside `src-tauri`, and `node scripts/claude-helper/smoke.mjs [bundled-helper-path]` after staging/bundling. Release CI runs the same smoke after signing both executables and the app. The smoke checks signed-out status, rejection of unauthenticated catalogs, and SDK initialization via `supportedModels()` without a generation request. Its packaging-only `probe` action creates a separate temporary home/config, refuses an authenticated account, and disables nonessential traffic; it cannot use the caller's account or bypass the catalog/prompt authentication gates. The smoke runner cleans up the entire child process group after each check, including timeouts.

Manual local-app checks: first verify both bundled executables retain execute permissions, obtain permission to stop the older app, and back up `settings.json`; connect Claude in the browser, cancel/retry, fetch both provider catalogs, select Default vs its separate concrete target in both tabs, send a chat, generate an insight (including chat during a long insight), restart and confirm persistence, disconnect/reconnect, and check the picker in a 360 x 480 window. If Fable is offered, also test a generation with it; any required provider consent is reported as an error, not silently bypassed. These authenticated/browser and visual checks are not replaced by unit tests.

To return to the older installed app, quit the test app first. Preserve the test app's current settings separately if needed, then restore `settings.json.pre-model-selection.bak` to `settings.json` before starting the old app. On macOS both files are under `~/Library/Application Support/expotify/`. The backup restores pre-migration settings, so later test-session setting or memory changes will not be present in the old app. Never launch the old app against the new settings format.
