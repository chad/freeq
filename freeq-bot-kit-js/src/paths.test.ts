/** Unit tests for paths.ts. */
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, stat, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { botDir, FREEQ_BOTS_ROOT } from "./paths.js";

describe("FREEQ_BOTS_ROOT", () => {
  it("anchors at ~/.freeq/bots", () => {
    expect(FREEQ_BOTS_ROOT.endsWith(`${sep}.freeq${sep}bots`)).toBe(true);
  });
});

describe("botDir", () => {
  let root: string;
  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), "freeq-bot-kit-paths-"));
  });
  afterEach(async () => {
    await rm(root, { recursive: true, force: true });
  });

  it("rejects empty names", async () => {
    await expect(botDir("", { root })).rejects.toThrow(/non-empty/);
  });

  it("rejects names with path separators", async () => {
    await expect(botDir("foo/bar", { root })).rejects.toThrow(/path separators/);
    await expect(botDir("foo\\bar", { root })).rejects.toThrow(/path separators/);
  });

  it("creates the directory under the given root", async () => {
    const dir = await botDir("test-bot", { root });
    expect(dir).toBe(join(root, "test-bot"));
    const s = await stat(dir);
    expect(s.isDirectory()).toBe(true);
  });

  it("is idempotent on repeated calls", async () => {
    const a = await botDir("test-bot", { root });
    const b = await botDir("test-bot", { root });
    expect(a).toBe(b);
    const s = await stat(a);
    expect(s.isDirectory()).toBe(true);
  });

  it("creates a fresh nested subdir without polluting peers", async () => {
    await botDir("a", { root });
    await botDir("b", { root });
    const sa = await stat(join(root, "a"));
    const sb = await stat(join(root, "b"));
    expect(sa.isDirectory()).toBe(true);
    expect(sb.isDirectory()).toBe(true);
  });

  it("uses 0700 permissions where the filesystem supports them", async () => {
    const dir = await botDir("perm-test", { root });
    const s = await stat(dir);
    // On Linux/macOS the mode bits include 0700. On filesystems that ignore
    // POSIX modes (e.g. some FAT mounts) this would be a different value;
    // skip the assertion in that case rather than fail spuriously.
    if (process.platform === "linux" || process.platform === "darwin") {
      expect(s.mode & 0o777).toBe(0o700);
    }
  });
});
