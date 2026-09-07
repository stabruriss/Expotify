import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
if (process.platform !== "darwin" || !["arm64", "x64"].includes(process.arch)) {
  throw new Error("Expotify's bundled Claude runtime currently supports macOS arm64 and x64");
}
const target = process.env.TAURI_ENV_TARGET_TRIPLE;
if (target && !target.startsWith(process.arch === "arm64" ? "aarch64-apple-" : "x86_64-apple-")) {
  throw new Error("Build Claude's native runtime on a matching macOS architecture");
}
const sdkDir = path.dirname(require.resolve("@anthropic-ai/claude-agent-sdk"));
const sdk = JSON.parse(readFileSync(path.join(sdkDir, "package.json"), "utf8"));
const nativeDir = path.dirname(require.resolve(`@anthropic-ai/claude-agent-sdk-darwin-${process.arch}/package.json`));
const native = JSON.parse(readFileSync(path.join(nativeDir, "package.json"), "utf8"));
if (sdk.version !== native.version) throw new Error("Claude SDK/native runtime versions do not match");
if (!readFileSync(path.join(nativeDir, "claude")).includes(Buffer.from("[Bootstrap] Cache unchanged, skipping write"))) {
  throw new Error("Claude bootstrap protocol changed; review catalog freshness before updating the bundled CLI");
}
const outputDir = path.join(root, "binaries", "claude");
mkdirSync(outputDir, { recursive: true });
const bun = process.env.BUN_BIN || "bun";
const result = spawnSync(bun, ["build", "--compile", "--minify", "--target=bun-darwin-" + process.arch,
  path.join(root, "scripts/claude-helper/runner.mjs"), "--outfile", path.join(outputDir, "expotify-claude-helper")],
{ cwd: root, stdio: "inherit" });
if (result.error) throw new Error(`Bun is required on the build machine only: ${result.error.message}`);
if (result.status !== 0) process.exit(result.status || 1);
copyFileSync(path.join(nativeDir, "claude"), path.join(outputDir, "claude"));
copyFileSync(path.join(sdkDir, "README.md"), path.join(outputDir, "SDK-README.md"));
chmodSync(path.join(outputDir, "claude"), 0o755);
chmodSync(path.join(outputDir, "expotify-claude-helper"), 0o755);
console.log(`Bundled Claude SDK ${sdk.version} / CLI ${sdk.claudeCodeVersion} (${process.arch})`);
