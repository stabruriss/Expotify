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

function summarizeAccountInfo(account) {
  if (!account) return null;

  const parts = [];
  if (account.email) parts.push(`email=${account.email}`);
  if (account.organization) parts.push(`org=${account.organization}`);
  if (account.subscriptionType) parts.push(`subscription=${account.subscriptionType}`);
  if (account.tokenSource) parts.push(`tokenSource=${account.tokenSource}`);
  if (account.apiKeySource) parts.push(`apiKeySource=${account.apiKeySource}`);
  return parts.join(", ");
}

function summarizeRateLimitInfo(info) {
  if (!info) return null;

  const parts = [];
  if (info.status) parts.push(`status=${info.status}`);
  if (info.rateLimitType) parts.push(`type=${info.rateLimitType}`);
  if (typeof info.utilization === "number") parts.push(`utilization=${info.utilization}`);
  if (info.overageStatus) parts.push(`overageStatus=${info.overageStatus}`);
  if (info.overageDisabledReason) {
    parts.push(`overageDisabledReason=${info.overageDisabledReason}`);
  }
  if (info.isUsingOverage !== undefined) parts.push(`isUsingOverage=${info.isUsingOverage}`);
  if (info.resetsAt) parts.push(`resetsAt=${info.resetsAt}`);
  if (info.overageResetsAt) parts.push(`overageResetsAt=${info.overageResetsAt}`);
  return parts.join(", ");
}

function summarizeAuthStatusMessage(msg) {
  const parts = [`isAuthenticating=${msg.isAuthenticating}`];
  if (Array.isArray(msg.output) && msg.output.length > 0) {
    parts.push(`output=${msg.output.join(" / ")}`);
  }
  if (msg.error) {
    parts.push(`error=${msg.error}`);
  }
  return parts.join(", ");
}

function formatDiagnosticError(error, diagnostics) {
  const parts = [error instanceof Error ? error.message : String(error)];

  const account = summarizeAccountInfo(diagnostics.account);
  if (account) parts.push(`account=${account}`);
  if (diagnostics.accountError) parts.push(`accountError=${diagnostics.accountError}`);
  if (diagnostics.assistantErrors.length > 0) {
    parts.push(`assistantErrors=${diagnostics.assistantErrors.join(",")}`);
  }
  if (diagnostics.resultErrors.length > 0) {
    parts.push(`resultErrors=${diagnostics.resultErrors.join(" ; ")}`);
  }
  const rateLimit = summarizeRateLimitInfo(diagnostics.rateLimitInfo);
  if (rateLimit) parts.push(`rateLimit=${rateLimit}`);
  if (diagnostics.authStatusMessages.length > 0) {
    parts.push(`authStatus=${diagnostics.authStatusMessages.join(" || ")}`);
  }
  if (diagnostics.messageFlow.length > 0) {
    parts.push(`messageFlow=${diagnostics.messageFlow.join(",")}`);
  }

  return parts.join(" | ");
}

async function runPrompt(request) {
  if (!request?.oauthToken) {
    throw new Error("Claude OAuth token is missing");
  }
  if (!request?.model) {
    throw new Error("Claude model is missing");
  }

  const options = buildOptions(request);
  const session = query({ prompt: request.prompt, options });
  let text = "";
  let structured = undefined;
  const diagnostics = {
    account: null,
    accountError: null,
    assistantErrors: [],
    resultErrors: [],
    rateLimitInfo: null,
    authStatusMessages: [],
    messageFlow: [],
  };

  try {
    try {
      diagnostics.account = await session.accountInfo();
    } catch (error) {
      diagnostics.accountError = error instanceof Error ? error.message : String(error);
    }

    for await (const msg of session) {
      diagnostics.messageFlow.push(
        msg.type === "result" ? `result:${msg.subtype}` : msg.type
      );

      if (msg.type === "assistant") {
        if (msg.error) {
          diagnostics.assistantErrors.push(msg.error);
          throw new Error(`Claude assistant error: ${msg.error}`);
        }
        for (const block of msg.message.content) {
          if (block.type === "text") {
            text += block.text;
          }
        }
        continue;
      }

      if (msg.type === "auth_status") {
        diagnostics.authStatusMessages.push(summarizeAuthStatusMessage(msg));
        continue;
      }

      if (msg.type === "rate_limit_event") {
        diagnostics.rateLimitInfo = msg.rate_limit_info;
        continue;
      }

      if (msg.type === "result") {
        if (msg.subtype !== "success") {
          diagnostics.resultErrors.push(...(msg.errors ?? []));
          throw new Error(msg.errors?.join("; ") || "Claude query failed");
        }
        structured = msg.structured_output;
      }
    }
  } catch (error) {
    throw new Error(formatDiagnosticError(error, diagnostics));
  } finally {
    session.close();
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
