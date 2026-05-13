/** Tests for the DID allowlist + per-DID action grants.
 *
 * Allowlist controls which non-owner DIDs can DM the bot and what IRC
 * actions each can drive. Owner is always implicitly allowed with the
 * default OWNER_ACTIONS set. */

import { describe, it, expect } from "vitest";
import {
  isAllowed,
  actionsFor,
  migrateAction,
  OWNER_ACTIONS,
  type AllowlistEntry,
} from "./allowlist.js";

const OWNER = "did:plc:owner";
const FRIEND = "did:plc:friend";
const STRANGER = "did:plc:stranger";

describe("isAllowed", () => {
  it("owner is always allowed even with an empty allowlist", () => {
    expect(isAllowed(OWNER, OWNER, [])).toBe(true);
  });

  it("non-owner is allowed when their DID is on the allowlist", () => {
    const list: AllowlistEntry[] = [{ did: FRIEND, label: "friend" }];
    expect(isAllowed(FRIEND, OWNER, list)).toBe(true);
  });

  it("non-owner not on the allowlist is rejected", () => {
    const list: AllowlistEntry[] = [{ did: FRIEND }];
    expect(isAllowed(STRANGER, OWNER, list)).toBe(false);
  });
});

describe("actionsFor", () => {
  it("owner gets the full OWNER_ACTIONS set", () => {
    const got = actionsFor(OWNER, OWNER, []);
    expect(got).toEqual([...OWNER_ACTIONS]);
  });

  it("allowlisted DID with no actions = chat-only (empty action set)", () => {
    const list: AllowlistEntry[] = [{ did: FRIEND, label: "friend" }];
    expect(actionsFor(FRIEND, OWNER, list)).toEqual([]);
  });

  it("allowlisted DID gets its declared actions", () => {
    const list: AllowlistEntry[] = [
      { did: FRIEND, actions: ["join", "privmsg-user"] },
    ];
    expect(actionsFor(FRIEND, OWNER, list)).toEqual(["join", "privmsg-user"]);
  });

  it("non-allowlisted, non-owner DID gets no actions (defense in depth)", () => {
    expect(actionsFor(STRANGER, OWNER, [])).toEqual([]);
  });
});

describe("migrateAction — legacy action-name compatibility", () => {
  // Pre-H-2-split allowlist.json files have bare "privmsg" / "notice"
  // actions. The migration loosens those to -user variants only — broadcast
  // to channels needs an explicit re-grant from the operator.

  it("maps legacy 'privmsg' to 'privmsg-user' (not -channel)", () => {
    expect(migrateAction("privmsg")).toEqual(["privmsg-user"]);
  });

  it("maps legacy 'notice' to 'notice-user' (not -channel)", () => {
    expect(migrateAction("notice")).toEqual(["notice-user"]);
  });

  it("passes scoped action names through unchanged", () => {
    expect(migrateAction("privmsg-user")).toEqual(["privmsg-user"]);
    expect(migrateAction("privmsg-channel")).toEqual(["privmsg-channel"]);
    expect(migrateAction("notice-user")).toEqual(["notice-user"]);
    expect(migrateAction("notice-channel")).toEqual(["notice-channel"]);
  });

  it("passes other actions through unchanged", () => {
    expect(migrateAction("join")).toEqual(["join"]);
    expect(migrateAction("part")).toEqual(["part"]);
    expect(migrateAction("nick")).toEqual(["nick"]);
  });

  it("does NOT silently upgrade legacy 'privmsg' to the channel-broadcast form", () => {
    // Security: an upgrading user with `"privmsg"` in their allowlist
    // must not suddenly be able to broadcast to arbitrary channels.
    expect(migrateAction("privmsg")).not.toContain("privmsg-channel");
    expect(migrateAction("notice")).not.toContain("notice-channel");
  });
});

describe("OWNER_ACTIONS — sanity check that defaults are safe", () => {
  it("does not include unscoped 'privmsg' or 'notice'", () => {
    expect(OWNER_ACTIONS).not.toContain("privmsg");
    expect(OWNER_ACTIONS).not.toContain("notice");
  });

  it("does not include 'nick' (too easy to weaponize via prompt-injection)", () => {
    expect(OWNER_ACTIONS).not.toContain("nick");
  });

  it("does not grant channel-wide broadcast actions by default", () => {
    expect(OWNER_ACTIONS).not.toContain("privmsg-channel");
    expect(OWNER_ACTIONS).not.toContain("notice-channel");
  });
});
