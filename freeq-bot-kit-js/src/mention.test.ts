// Tests for the pure mention-matching helper. The stateful wrapper (the
// per-channel cooldown, the live nick read) lives on FreeqBot and is
// covered in bot.test.ts.

import { describe, it, expect } from "vitest";
import { matchMention } from "./mention.js";

describe("matchMention: @<nick> anywhere triggers", () => {
  it("matches @<nick> at start", () => {
    expect(matchMention("yokota", "@yokota help")).toEqual({ stripped: "help" });
  });

  it("matches @<nick> mid-message", () => {
    expect(matchMention("yokota", "hey @yokota help")).toEqual({
      stripped: "hey help",
    });
  });

  it("matches @<nick> at end (no trailing content)", () => {
    expect(matchMention("yokota", "ping @yokota")).toEqual({ stripped: "ping" });
  });

  it("is case-insensitive on the nick", () => {
    expect(matchMention("yokota", "@YOKOTA help")).toEqual({ stripped: "help" });
    expect(matchMention("YOKOTA", "@yokota help")).toEqual({ stripped: "help" });
  });

  it("collapses extra whitespace introduced by stripping the address", () => {
    expect(matchMention("yokota", "hey    @yokota    help me")).toEqual({
      stripped: "hey help me",
    });
  });
});

describe("matchMention: <nick>:/, addressing", () => {
  it("matches <nick>: at start", () => {
    expect(matchMention("yokota", "yokota: help")).toEqual({ stripped: "help" });
  });

  it("matches <nick>, at start", () => {
    expect(matchMention("yokota", "yokota, help")).toEqual({ stripped: "help" });
  });

  it("matches <nick>: mid-message", () => {
    expect(matchMention("yokota", "hey yokota: help me")).toEqual({
      stripped: "hey help me",
    });
  });

  it("matches <nick>, mid-message", () => {
    expect(matchMention("yokota", "hey yokota, what's up")).toEqual({
      stripped: "hey what's up",
    });
  });
});

describe("matchMention: ignore cases", () => {
  it("does not match bare nick without @ or :/,", () => {
    expect(matchMention("yokota", "yokota wrote a great thing")).toBeNull();
    expect(matchMention("yokota", "I'll ask yokota about that")).toBeNull();
  });

  it("does not match @<nick> embedded in an email-like token", () => {
    expect(matchMention("yokota", "email me at user@yokota.com")).toBeNull();
  });

  it("does not match <nick>: when it's part of a larger word", () => {
    // "yokotabot:" — colon is there but the nick isn't a whole word.
    expect(matchMention("yokota", "yokotabot: hi")).toBeNull();
  });

  it("returns null for empty / whitespace-only text", () => {
    expect(matchMention("yokota", "")).toBeNull();
    expect(matchMention("yokota", "   ")).toBeNull();
  });

  it("returns null when nick is empty (defensive)", () => {
    expect(matchMention("", "anything @ here")).toBeNull();
  });
});

describe("matchMention: regex-special chars in the nick", () => {
  it("escapes dots", () => {
    expect(matchMention("u.s.a", "@u.s.a hello")).toEqual({ stripped: "hello" });
    // Should NOT trigger on a different nick matching the unescaped regex.
    expect(matchMention("u.s.a", "@uXsXa hello")).toBeNull();
  });

  it("escapes dashes (common in bot nicks)", () => {
    expect(matchMention("zerosum-agent", "@zerosum-agent hello")).toEqual({
      stripped: "hello",
    });
  });

  it("escapes underscores and brackets", () => {
    expect(matchMention("[admin]_bot", "[admin]_bot: help")).toEqual({
      stripped: "help",
    });
  });
});

describe("matchMention: result has only stripped (caller handles state)", () => {
  it("doesn't include kind/cooldown — that's the bot method's job", () => {
    const r = matchMention("yokota", "@yokota hi");
    expect(r).toEqual({ stripped: "hi" });
    expect("kind" in (r as object)).toBe(false);
    expect("cooldownMs" in (r as object)).toBe(false);
  });
});
