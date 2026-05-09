// Dispatch an incoming DM to a Claude Code subprocess.
//
// One persistent claude session per agent — first call mints a session,
// subsequent calls --resume it. Session id is persisted at
// ~/.freeqcc/session.json so it survives daemon restarts.
//
// Uses --output-format json so we get a structured reply with an
// authoritative session_id field, regardless of whether the underlying
// session got forked.
import { spawn } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";

export interface DispatchResult {
  reply: string;
  sessionId: string;
  durationMs: number;
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
