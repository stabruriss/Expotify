import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const readJson = file => JSON.parse(readFileSync(file, "utf8"));
const pkg = readJson("package.json");
const lock = readJson("package-lock.json");
const tauri = readJson("src-tauri/tauri.conf.json");
const cargo = JSON.parse(execFileSync("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1",
  "--manifest-path", "src-tauri/Cargo.toml"], { encoding: "utf8" }));
const versions = [pkg.version, lock.version, lock.packages[""].version, tauri.version,
  cargo.packages.find(item => item.name === "expotify")?.version];
assert.match(pkg.version, /^\d+\.\d+\.\d+$/);
assert.ok(versions.every(version => version === pkg.version), `Version mismatch: ${versions.join(", ")}`);
const tag = process.argv[2] || `v${pkg.version}`;
assert.equal(tag, `v${pkg.version}`, "Release tag must match every app version");
const notes = readFileSync(`.github/release-notes/${tag}.md`, "utf8");
for (const heading of ["English", "中文", "日本語"]) {
  const section = notes.split(`### ${heading}\n`)[1]?.split(/\n#{1,3} /)[0];
  assert.ok(section && /^- \S/m.test(section), `Missing release notes for ${heading}`);
}
console.log(`PASS: ${tag} versions and all three release-note languages agree.`);
