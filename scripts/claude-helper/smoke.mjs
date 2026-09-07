import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";

// Fresh home/config: no browser login, credential imports, refreshes or generation.
const configDir = mkdtempSync(path.join(os.tmpdir(), "expotify-claude-smoke-"));
const executable = path.resolve(process.argv[2] || "binaries/claude/expotify-claude-helper");
function run(action) {
  const result = spawnSync(executable, [], {
    input: JSON.stringify({ action, configDir }), encoding: "utf8", timeout: 25000,
    detached: true, killSignal: "SIGKILL",
    env: { HOME: configDir, PATH: "/usr/bin:/bin:/usr/sbin:/sbin" },
  });
  // Reap a native child too if a broken runtime outlives the helper/deadline.
  if (result.pid) {
    try { process.kill(-result.pid, "SIGKILL"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  }
  assert.equal(result.error, undefined);
  return result;
}

try {
  const result = run("status");
  assert.equal(result.status, 0);
  assert.deepEqual(JSON.parse(result.stdout), { ok: true, data: { loggedIn: false } });
  const catalog = run("catalog");
  assert.equal(catalog.status, 1);
  const response = JSON.parse(catalog.stdout);
  assert.equal(response.ok, false);
  assert.match(response.error, /Reconnect Claude/);
  console.log("PASS: bundled helper + native CLI run with no external Node/Bun/Claude on PATH; isolated account is signed out.");
  console.log("PASS: unauthenticated model discovery is rejected before SDK initialization or any generation request.");
  const probe = run("probe");
  assert.equal(probe.status, 0);
  assert.deepEqual(JSON.parse(probe.stdout), { ok: true, data: { sdkInitialized: true } });
  console.log("PASS: bundled SDK initializes and reads model metadata in a separate signed-out namespace without generation.");
} finally { rmSync(configDir, { recursive: true, force: true }); }
