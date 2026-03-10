import { query } from "@anthropic-ai/claude-agent-sdk";
import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";

const require = createRequire(import.meta.url);
const sdkEntry = require.resolve("@anthropic-ai/claude-agent-sdk");
const sdkDir = path.dirname(sdkEntry);
const cliPath = path.join(sdkDir, "cli.js");

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function buildOptions(request) {
  return {
    model: request.model,
    maxTurns: 1,
    systemPrompt: request.systemPrompt,
    tools: [],
    allowedTools: [],
    permissionMode: "dontAsk",
    persistSession: false,
    cwd: process.cwd(),
    executable: process.execPath,
    pathToClaudeCodeExecutable: cliPath,
    env: {
      ...process.env,
      ANTHROPIC_API_KEY: "",
      CLAUDE_CODE_OAUTH_TOKEN: request.oauthToken,
      CLAUDE_AGENT_SDK_CLIENT_APP: "expotify/0.5.2",
    },
  };
}

async function runPrompt(request) {
  if (!request?.oauthToken) {
    throw new Error("Claude OAuth token is missing");
  }
  if (!request?.model) {
    throw new Error("Claude model is missing");
  }

  const options = buildOptions(request);
  let text = "";
  let structured = undefined;

  for await (const msg of query({ prompt: request.prompt, options })) {
    if (msg.type === "assistant") {
      if (msg.error) {
        throw new Error(`Claude assistant error: ${msg.error}`);
      }
      for (const block of msg.message.content) {
        if (block.type === "text") {
          text += block.text;
        }
      }
      continue;
    }

    if (msg.type === "result") {
      if (msg.subtype !== "success") {
        throw new Error(msg.errors?.join("; ") || "Claude query failed");
      }
      structured = msg.structured_output;
    }
  }

  return {
    text: text.trim(),
    structured: structured ?? null,
  };
}

async function main() {
  const raw = await readStdin();
  const request = JSON.parse(raw);
  const response = await runPrompt(request);
  process.stdout.write(`${JSON.stringify(response)}\n`);
}

main().catch((error) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
});
