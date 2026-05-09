// Dispatch an incoming DM to a Claude Code subprocess.
//
// One persistent claude session per agent — first call mints a session,
// subsequent calls --resume it. Session id is persisted at
// ~/.freeqcc/session.json so it survives daemon restarts.
//
// Two entry points:
//   dispatchToClaude(text)              — non-streaming; one shot, returns final reply
//   dispatchToClaudeStreaming(text, h)  — streaming; emits chunks as Claude produces them
import { spawn } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";

export interface DispatchResult {
  reply: string;
  sessionId: string;
  durationMs: number;
}

/** Callbacks for streaming dispatch. Each fires on the daemon's event loop. */
export interface StreamHandlers {
  /** Latest accumulated text the model has produced so far. */
  onChunk: (accumulated: string) => void;
  /** Final text + session metadata. After this, no more onChunk fires. */
  onComplete: (final: string, sessionId: string, durationMs: number, costUsd?: number) => void;
  /** Subprocess crashed, claude returned non-zero, or output stream broke. */
  onError?: (err: Error) => void;
}

interface SessionFile {
  sessionId: string;
  lastUsedAt: string; // ISO timestamp
}

async function loadSession(): Promise<string | null> {
  try {
    const raw = await readFile(paths.session, "utf8");
    const parsed = JSON.parse(raw) as SessionFile;
    return parsed.sessionId ?? null;
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return null;
    return null; // bad file — treat as no session and let next dispatch mint a new one
  }
}

async function saveSession(sessionId: string): Promise<void> {
  await ensureDir();
  const data: SessionFile = { sessionId, lastUsedAt: new Date().toISOString() };
  await writeFile(paths.session, JSON.stringify(data, null, 2) + "\n", { mode: 0o600 });
}

/**
 * Run `claude -p` with the given message. If a session id is cached,
 * pass --resume <id>. Returns the reply text plus the (possibly new)
 * session id, which we persist for the next call.
 */
export async function dispatchToClaude(text: string): Promise<DispatchResult> {
  const sessionId = await loadSession();
  const args = ["--print", "--output-format", "json"];
  if (sessionId) {
    args.push("--resume", sessionId);
  }
  args.push(text);

  const startedAt = Date.now();
  const out = await runClaude(args);
  const durationMs = Date.now() - startedAt;

  // Parse the JSON envelope. Claude's --output-format json shape includes
  // at minimum { result, session_id } though field names have varied;
  // fall back across a couple of likely names so we're tolerant.
  let envelope: Record<string, unknown> | null = null;
  try {
    envelope = JSON.parse(out.trim()) as Record<string, unknown>;
  } catch {
    // No JSON envelope — treat the whole stdout as the reply, mint a
    // best-guess session id (we'll replace it on the next successful
    // structured response).
    const fallbackSession = sessionId ?? "unknown";
    return { reply: out.trim(), sessionId: fallbackSession, durationMs };
  }

  const reply =
    (envelope.result as string | undefined) ??
    (envelope.response as string | undefined) ??
    (envelope.text as string | undefined) ??
    "";
  const newSessionId =
    (envelope.session_id as string | undefined) ??
    (envelope.sessionId as string | undefined) ??
    sessionId ??
    "";

  if (newSessionId) {
    await saveSession(newSessionId);
  }
  return { reply: reply.trim(), sessionId: newSessionId, durationMs };
}

function runClaude(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const proc = spawn("claude", args, {
      stdio: ["ignore", "pipe", "pipe"],
      env: process.env,
    });
    let stdout = "";
    let stderr = "";
    proc.stdout.on("data", (chunk: Buffer) => {
      stdout += chunk.toString("utf8");
    });
    proc.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString("utf8");
    });
    proc.on("error", (err) => reject(err));
    proc.on("close", (code) => {
      if (code === 0) {
        resolve(stdout);
      } else {
        reject(
          new Error(
            `claude exited with code ${code}\nstderr: ${stderr.slice(0, 500)}`,
          ),
        );
      }
    });
  });
}

/**
 * Streaming dispatch. Spawns `claude --print --output-format stream-json
 * --verbose --include-partial-messages [--resume <id>]`, parses each line as
 * JSON, and fires:
 *   - onChunk(accumulated) on every text_delta event (caller usually wants
 *     to throttle these to avoid spamming downstream)
 *   - onComplete(final, sessionId, durationMs, costUsd) on the final
 *     {"type":"result"} record
 *   - onError(err) if the process crashes or exits non-zero
 *
 * Persists the returned session_id so the next call can --resume.
 */
export async function dispatchToClaudeStreaming(
  text: string,
  handlers: StreamHandlers,
): Promise<void> {
  const sessionId = await loadSession();
  const args = [
    "--print",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
  ];
  if (sessionId) {
    args.push("--resume", sessionId);
  }
  args.push(text);

  const startedAt = Date.now();
  let accumulated = "";

  await new Promise<void>((resolve) => {
    const proc = spawn("claude", args, {
      stdio: ["ignore", "pipe", "pipe"],
      env: process.env,
    });
    let stdoutBuffer = "";
    let stderrBuffer = "";

    const consumeLine = (line: string): void => {
      if (!line) return;
      let evt: Record<string, unknown>;
      try {
        evt = JSON.parse(line) as Record<string, unknown>;
      } catch {
        return; // skip malformed
      }
      // text_delta on a stream_event: append + chunk callback
      if (evt.type === "stream_event") {
        const inner = evt.event as { type?: string; delta?: { type?: string; text?: string } } | undefined;
        if (
          inner?.type === "content_block_delta" &&
          inner.delta?.type === "text_delta" &&
          typeof inner.delta.text === "string"
        ) {
          accumulated += inner.delta.text;
          try {
            handlers.onChunk(accumulated);
          } catch (cbErr) {
            // Caller's onChunk threw — log + carry on
            handlers.onError?.(cbErr as Error);
          }
        }
      }
      // result: final
      if (evt.type === "result" && evt.subtype === "success") {
        const final = (evt.result as string | undefined) ?? accumulated;
        const newSessionId =
          (evt.session_id as string | undefined) ?? sessionId ?? "";
        const cost = evt.total_cost_usd as number | undefined;
        const durationMs = Date.now() - startedAt;
        if (newSessionId) {
          // fire and forget; we're inside a closure
          void saveSession(newSessionId);
        }
        try {
          handlers.onComplete(final, newSessionId, durationMs, cost);
        } catch (cbErr) {
          handlers.onError?.(cbErr as Error);
        }
      }
    };

    proc.stdout.on("data", (chunk: Buffer) => {
      stdoutBuffer += chunk.toString("utf8");
      let nl;
      while ((nl = stdoutBuffer.indexOf("\n")) >= 0) {
        const line = stdoutBuffer.slice(0, nl).trim();
        stdoutBuffer = stdoutBuffer.slice(nl + 1);
        consumeLine(line);
      }
    });
    proc.stderr.on("data", (chunk: Buffer) => {
      stderrBuffer += chunk.toString("utf8");
    });
    proc.on("error", (err) => {
      handlers.onError?.(err);
      resolve();
    });
    proc.on("close", (code) => {
      if (stdoutBuffer.trim()) {
        consumeLine(stdoutBuffer.trim());
      }
      if (code !== 0) {
        handlers.onError?.(
          new Error(
            `claude exited with code ${code}\nstderr: ${stderrBuffer.slice(0, 500)}`,
          ),
        );
      }
      resolve();
    });
  });
}
