# Freeq Web App — Design Plan

## Vision

A modern team communication app — comparable to Slack or Discord in feel — built on freeq's IRC+AT Protocol infrastructure. AT Protocol identity only (no guest mode in this client). Slash commands work but are never required. Every action has a GUI affordance. The protocol stays clean; the polish lives in the client.

This document covers: what already works, what the client must do, what protocol enhancements are needed, and a build plan.

---

## 1. What Already Exists (Protocol Inventory)

### Ready to use — no protocol changes needed

| Feature | Protocol support | Notes |
|---------|-----------------|-------|
| Auth (Bluesky login) | SASL ATPROTO-CHALLENGE via OAuth | Server-side OAuth flow exists at `/auth/login` + `/auth/callback` |
| Channels (join/part/create) | JOIN/PART | Default +nt on creation |
| Messages | PRIVMSG/NOTICE over WebSocket | Full IRCv3 tag support |
| DMs | PRIVMSG to nick | Cross-server via S2S relay |
| History on join | BATCH chathistory | Last 100 messages replayed |
| On-demand history | CHATHISTORY LATEST/BEFORE/AFTER | Paginated, server-time tagged |
| Topic | TOPIC command + 332 numeric | +t enforcement |
| Channel modes | +i +t +k +n +m +b +o +v | Full set |
| Kick/Ban | KICK, MODE +b | DID-based bans survive nick changes |
| Invite | INVITE | +i channels |
| Nick changes | NICK | Broadcast to shared channels |
| Reactions | TAGMSG with `+react` tag | Fallback ACTION for plain clients |
| Media attachments | IRCv3 tags: `media-url`, `content-type`, `media-alt` | Uploaded to AT Protocol PDS |
| Link previews | `text/x-link-preview` content type | Server-side OG fetching via SDK |
| User info | WHOIS (DID, handle, iroh endpoint) | Numerics 311/330/671/672 |
| User list | NAMES (353/366) with @/+ prefixes | |
| Channel list | LIST (322/323) with topics | |
| Online check | ISON | |
| Away status | AWAY (301/305/306) | RPL_AWAY on PM |
| Echo | `echo-message` cap | Server echoes own messages |
| Account notify | `account-notify` + `extended-join` | DID broadcast on auth/join |
| REST API | `/api/v1/health`, channels, history, users | Read-only, JSON |
| E2EE | ENC1 (passphrase) + ENC2 (DID-based) | Server-transparent |
| Server-time | `server-time` cap | Timestamps on all messages |
| Multi-prefix | `multi-prefix` cap | Accurate NAMES prefixes |

### Gaps requiring protocol enhancements

| Need | Current state | Required enhancement |
|------|--------------|---------------------|
| **Typing indicators** | TAGMSG exists but no typing tag convention | Define `+typing` tag (client-only, no server storage) |
| **Read receipts / read position** | Nothing | Define `+read` TAGMSG with `msgid`; or server-side read marker via new command |
| **Message IDs** | Not implemented | Add `msgid` IRCv3 cap — server assigns unique ID to each message |
| **Message editing** | Not possible | TAGMSG with `+draft/edit` referencing original `msgid` |
| **Message deletion** | Not possible | TAGMSG with `+draft/delete` referencing original `msgid` |
| **Threads / replies** | Nothing | `+reply` tag with parent `msgid` |
| **User profiles** | WHOIS only (synchronous) | REST endpoint already exists. Client can also resolve AT Protocol profile (avatar, display name, bio) directly from PDS |
| **Presence (online/idle/offline)** | AWAY exists but no idle tracking | Expose idle time in WHO/WHOIS; consider `away-notify` cap |
| **Unread counts / badges** | Client must track locally | Server-side read markers (or client-side IndexedDB) |
| **Search** | No FTS | Add server-side `SEARCH` command or REST `/api/v1/search?q=` backed by SQLite FTS5 |
| **File uploads** | SDK uploads to PDS, sends URL in tag | Works — but client needs drag-and-drop UX and progress |
| **Pinned messages** | Not implemented | Channel metadata: `+pin` TAGMSG or new MODE variant |
| **Bookmarks** | Not implemented | Client-local (IndexedDB) — no protocol needed |
| **User groups / mentions** | No @-group concept | Client-side only (expansion before send) |
| **Message signing** | P0 in TODO but not yet implemented | `+freeq.at/sig` tag on every message from DID-auth users |

---

## 2. Protocol Enhancements (Approval Required)

These additions are backward-compatible. Old clients ignore unknown tags. None change existing behavior.

### 2.1 `msgid` — Message IDs (Required)

**IRCv3 spec exists for this.** Server assigns a unique ID (ULID or UUID) to each message. Carried in the `msgid` tag.

```
@msgid=01HXYZ...;time=2026-02-18T15:00:00Z :alice!a@host PRIVMSG #team :hello
```

**Why required:** Editing, deletion, replies, reactions-to-specific-message, and read markers all need to reference a specific message. Without this, none of the modern UX features work. This is the single most important protocol addition.

**Server work:** ~30 lines. Generate ULID in PRIVMSG/NOTICE handler, attach as tag, store in DB.

**Backward compat:** Old clients ignore the tag. CHATHISTORY replay includes msgid.

### 2.2 `+typing` — Typing Indicators (Client-only)

```
@+typing=active TAGMSG #team
@+typing=done TAGMSG #team
```

**Why:** Core modern chat UX. "Alice is typing..." in the input area.

**Server work:** Zero. TAGMSG already relays tags. Server doesn't store these. The `+` prefix means it's a client-only tag per IRCv3 spec.

**Privacy:** Only sent when the user is actively typing in a channel they've joined. Opt-out in client settings. The server never generates these.

### 2.3 `+reply` — Threaded Replies

```
@+reply=01HXYZ... :alice!a@host PRIVMSG #team :I agree with this
```

**Why:** Lets users respond to specific messages without quoting. The client renders these as threaded or inline with a "replying to..." header.

**Server work:** Zero — it's a client tag, server passes it through. History replay preserves tags. Client resolves the referenced msgid from its local message cache.

**Backward compat:** Old clients see a normal message. The reply context is invisible to them (acceptable — same as how Discord/Slack replies appear in bridges).

### 2.4 `+draft/edit` and `+draft/delete` — Message Editing and Deletion

Follows the [IRCv3 draft](https://github.com/ircv3/ircv3-specifications/pull/524) pattern:

```
@+draft/edit=01HXYZ... :alice!a@host PRIVMSG #team :hello (fixed typo)
@+draft/delete=01HXYZ... TAGMSG #team
```

**Rules:**
- Only the original author can edit/delete
- Server enforces authorship (match DID or session)
- Server stores the edit as a new message with `replaces` field
- Clients that don't understand edits see a new message (acceptable degradation)
- Deletes are soft — server marks message as deleted, clients hide it

**Server work:** Medium. Need authorship check on incoming edit/delete, DB field for `replaces`/`deleted_at`, CHATHISTORY must return edits correctly. ~100 lines.

**Approval needed:** This is the most invasive change. Requires server-side enforcement. But the alternative (no editing) is a dealbreaker for a Slack replacement.

### 2.5 `+read` — Read Markers (Optional, can defer)

```
@+read=01HXYZ... TAGMSG #team
```

Client sends this to indicate they've read up to msgid `01HXYZ...`. Server can optionally store per-DID read position. Other clients for the same DID can sync unread state.

**Server work:** Small if stored. Zero if client-only (IndexedDB).

**Recommendation:** Start with client-local tracking (IndexedDB). Add server-side later when multi-device sync matters.

### 2.6 `away-notify` — Presence Updates

**IRCv3 spec exists.** Server broadcasts AWAY changes to shared channels.

```
:alice!a@host AWAY :In a meeting
:alice!a@host AWAY
```

**Server work:** Small. The server already tracks AWAY state. Just needs to broadcast on change to channel members who negotiated the cap. ~20 lines.

### 2.7 Search (Can defer to Phase 2)

Either:
- REST: `GET /api/v1/search?q=term&channel=#team&limit=50`
- IRC: `SEARCH #team :search terms` → batch of results

**Server work:** Wire up SQLite FTS5. Medium effort but not blocking for launch.

### 2.8 Message Signing (`+freeq.at/sig`)

Already P0 in TODO. Every message from a DID-authenticated user carries a cryptographic signature. The web app should:
- Display a "verified" indicator on signed messages
- Warn on unsigned messages in authenticated channels
- Allow clicking the indicator to see the signing DID

---

## 3. Client Architecture

### 3.1 Technology

| Choice | Rationale |
|--------|-----------|
| **React + TypeScript** | Largest talent pool, best component ecosystem, fast iteration |
| **Vite** | Fast builds, good DX |
| **Tailwind CSS** | Utility-first, consistent with modern design teams |
| **Zustand** | Minimal state management, no boilerplate |
| **IndexedDB (Dexie)** | Local message cache, offline search, read markers |
| **Separate repo** | `freeq-app/` or `github.com/chad/freeq-app`. Clean separation from infrastructure |

### 3.2 Connection Layer

The client speaks IRC over WebSocket, same as the existing `freeq-web/index.html`. But structured properly:

```
┌──────────────────────────────────────────────────┐
│                    React UI                       │
├──────────────────────────────────────────────────┤
│              Zustand Store (state)                │
│  channels, messages, users, presence, unreads     │
├──────────────────────────────────────────────────┤
│           IRC Protocol Adapter                    │
│  parse(), serialize(), cap negotiation, SASL      │
│  Translates IRC events → store actions            │
│  Translates UI actions → IRC commands             │
├──────────────────────────────────────────────────┤
│         WebSocket Transport                       │
│  Auto-reconnect, message queue, health ping       │
└──────────────────────────────────────────────────┘
```

The IRC Protocol Adapter is the key abstraction. The React UI never sees IRC protocol. It sees:

```typescript
// What the store exposes
interface Channel {
  name: string;
  topic: string;
  members: Member[];
  modes: Set<string>;
  unreadCount: number;
  mentionCount: number;
  messages: Message[];
}

interface Message {
  id: string;          // msgid
  from: Member;
  text: string;
  timestamp: Date;
  replyTo?: string;    // parent msgid
  editOf?: string;     // original msgid
  deleted: boolean;
  reactions: Map<string, Set<string>>;  // emoji → set of nicks
  media?: MediaAttachment;
  signature?: string;  // verified DID sig
  signed: boolean;
}

interface Member {
  nick: string;
  did?: string;
  handle?: string;
  displayName?: string;
  avatar?: string;     // from AT Protocol profile
  isOp: boolean;
  isVoiced: boolean;
  away?: string;
  typing: boolean;
}

// What the UI calls
interface Actions {
  sendMessage(channel: string, text: string, replyTo?: string): void;
  editMessage(channel: string, msgId: string, newText: string): void;
  deleteMessage(channel: string, msgId: string): void;
  react(channel: string, msgId: string, emoji: string): void;
  setTopic(channel: string, topic: string): void;
  joinChannel(channel: string): void;
  leaveChannel(channel: string): void;
  inviteUser(channel: string, nick: string): void;
  kickUser(channel: string, nick: string, reason?: string): void;
  setMode(channel: string, mode: string, arg?: string): void;
  uploadFile(channel: string, file: File): Promise<void>;
  startDM(nick: string): void;
  setAway(reason?: string): void;
}
```

The adapter layer translates bidirectionally. `sendMessage("team", "hello")` becomes `PRIVMSG #team :hello`. An incoming `@msgid=abc;time=... :alice PRIVMSG #team :hello` becomes a `Message` object in the store.

### 3.3 AT Protocol Profile Resolution

When a user is encountered (JOIN, NAMES, WHOIS), the client:

1. Gets DID from `extended-join` or `account-notify` or WHOIS 330
2. Resolves DID document (cached, 1hr TTL)
3. Fetches AT Protocol profile: `app.bsky.actor.getProfile` (public API, no auth needed)
4. Extracts: display name, avatar URL, bio
5. Caches in IndexedDB

This gives every authenticated user a rich profile card — avatar, display name, bio — without any protocol changes. The data comes from the AT Protocol social layer.

Guests (which this client doesn't support, but may appear via S2S from other servers) show a generic avatar and their nick.

---

## 4. UX Design

### 4.1 Layout

```
┌──────────────────────────────────────────────────────────┐
│  [freeq logo]  Server: irc.freeq.at  [🟢 connected]     │
│  Signed in as chadfowler.com (did:plc:...)   [@] [⚙️]    │
├────────┬─────────────────────────────────┬───────────────┤
│        │  #team                    ⚙️ 📌  │               │
│ DIRECT │  Topic: Sprint planning         │  MEMBERS (12) │
│ ────── │  ─────────────────────────────  │               │
│ @alice │  Alice Chen · 9:41 AM           │  👑 chad      │
│ @bob   │  Has anyone reviewed the PR?    │  ⭐ alice     │
│        │          ↩️ 👍2 ❤️1              │    bob        │
│ CHANS  │                                 │    carol      │
│ ────── │  Bob Smith · 9:42 AM            │               │
│ #team  │  ↪ replying to Alice            │  ── Away ──   │
│ #eng   │  Looking at it now              │    dave (mtg) │
│ #random│                                 │               │
│        │  ··· Carol is typing            │               │
│        ├─────────────────────────────────┤               │
│        │ [📎] Type a message...    [😀] [↵]│               │
└────────┴─────────────────────────────────┴───────────────┘
```

### 4.2 Sidebar

**Direct Messages** section at top:
- Shows recent DM conversations
- Avatar + display name (from AT Protocol profile)
- Unread badge
- Online/away/offline indicator dot

**Channels** section below:
- Channel name with unread badge
- Bold + red badge for mentions
- Muted channels in dimmer text
- "Browse channels" button (→ LIST command)
- "Create channel" button (→ JOIN with new name)

**Collapsible sections** — user can collapse DMs or channels.

### 4.3 Message Display

Each message shows:
- **Avatar** (from AT Protocol profile, or generated from DID)
- **Display name** (from AT Protocol, falls back to nick)
- **Handle** in lighter text (e.g. `@chadfowler.com`)
- **Timestamp** (relative: "9:41 AM", hover for absolute)
- **Verified badge** (🔒 if message is signed, hover shows signing DID)
- **Message text** with:
  - Markdown-lite rendering (bold, italic, code, code blocks)
  - Link previews (from `text/x-link-preview` tag or client-side OG fetch)
  - Image/video/audio embeds (from `media-url` tag)
  - @mentions highlighted
  - Channel links clickable (#channel → switch to it)
- **Reaction bar** below message (emoji counts, click to add/remove)
- **Reply indicator** ("↪ replying to Alice: Has anyone...") with click to scroll to original
- **Edited indicator** ("(edited)" with hover to see original)

**Hover actions** on each message (right side):
- 😀 React
- 💬 Reply
- ✏️ Edit (own messages only)
- 🗑️ Delete (own messages only)
- 📌 Pin (ops only)
- ⋯ More (copy link, copy text)

### 4.4 Compose Box

- Rich text area (not `<input>`)
- Paste images → auto-upload to PDS → send with media tags
- Drag and drop files → same upload flow
- `@` triggers member autocomplete popup
- `#` triggers channel autocomplete popup
- `:` triggers emoji picker
- Shift+Enter for newline (rendered with proper line breaks)
- Up arrow on empty input → edit last message
- Reply banner above input when replying (with cancel button)
- Typing indicator sent automatically (debounced, 3s interval)

### 4.5 Member List (Right Sidebar)

- Grouped by role: Operators (👑), Voiced (⭐), Members
- Each member shows: avatar, display name, away status
- Click → profile popover (DID, handle, bio, DM button, WHOIS info)
- Collapsible away section
- Online/away indicators (green dot / yellow dot)

### 4.6 Channel Settings (⚙️ button)

Modal or slide-out panel:
- **Topic** — editable by ops (or anyone if -t)
- **Modes** — toggle switches for +i, +t, +n, +m, +k
  - Each has a plain-language label: "Invite only", "Topic lock", etc.
- **Members** — list with op/voice/kick/ban actions
- **Bans** — list current bans with unban button
- **Notifications** — mute channel, mention-only, all messages

### 4.7 Login Flow

1. User lands on app (e.g. `app.freeq.at`)
2. Single input: "Enter your Bluesky handle" (e.g. `chadfowler.com`)
3. Click "Sign in with Bluesky" → OAuth redirect
4. Return → auto-connect to WebSocket → SASL auth → join default channels
5. No server URL input, no nick input, no manual configuration
   - Server URL from app config (or domain detection)
   - Nick derived from AT handle (e.g. `chadfowler` from `chadfowler.com`)
   - Default channels from server MOTD or app config

### 4.8 Notifications

- Browser Notification API for mentions and DMs (with permission prompt)
- Title bar badge: `(3) freeq` for unread count
- Sound (optional, off by default)
- Per-channel notification settings (all / mentions / none)
- Service Worker for push notifications when tab is backgrounded (Phase 2)

### 4.9 Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Cmd+K | Quick switcher (channels + DMs) |
| Cmd+Shift+A | Jump to unread |
| Alt+↑/↓ | Switch channels |
| Cmd+/ | Show all shortcuts |
| Esc | Close modal / cancel reply |
| Up (empty input) | Edit last message |

---

## 5. What The Client Handles (No Protocol Change)

Several "modern" features are purely client-side:

| Feature | Implementation |
|---------|---------------|
| **Markdown rendering** | Client parses `*bold*`, `_italic_`, `` `code` ``, ``` ```blocks``` ```. Sent as plain text — old clients see the raw markdown. This is how every Slack/Discord bridge works |
| **Message grouping** | Messages from same user within 2min shown without repeated avatar/name |
| **Unread tracking** | IndexedDB stores last-read msgid per channel. Computed locally |
| **Bookmarks** | IndexedDB. Channel-level mute/pin is local preference |
| **AT Protocol profiles** | Direct PDS API calls from browser. No server involvement |
| **Emoji picker** | Client-side component with Unicode emoji |
| **File previews** | Detect `media-url` tag, render appropriate embed |
| **@mention autocomplete** | Client has NAMES data, autocomplete locally |
| **Link previews** | If server provides `text/x-link-preview` tags, render them. Otherwise client-side OG fetch (with CORS limitations) |
| **Sound notifications** | Client-side audio |
| **Theme (light/dark)** | CSS variables, local preference |
| **Compact/comfortable density** | CSS toggle |

---

## 6. Build Plan

### Phase 0: Protocol Prerequisites (Server-side, ~1 week)

Must be done before the web app can launch with modern UX:

1. **`msgid` support** — Generate ULID for every PRIVMSG/NOTICE, attach as IRCv3 tag, store in DB, include in CHATHISTORY replay
2. **Message signing** — (Already P0 in TODO) `+freeq.at/sig` tag
3. **`away-notify` cap** — Broadcast AWAY changes to channel members

Nice to have for Phase 0 but can slip:
4. Edit/delete support (authorship check, DB schema, CHATHISTORY integration)

### Phase 1: Foundation (~2 weeks)

- Project scaffolding (Vite + React + TypeScript + Tailwind)
- IRC-over-WebSocket connection layer with auto-reconnect
- IRCv3 parser/serializer (TypeScript, from scratch — ~200 lines)
- CAP negotiation + SASL ATPROTO-CHALLENGE (OAuth flow)
- Zustand store with channel/message/member state
- Basic layout: sidebar, message list, compose box, member list
- Message rendering: text, timestamps, avatars (from AT Protocol)
- Join/part/create channel
- Send/receive messages
- CHATHISTORY loading (scroll-up pagination)

### Phase 2: Rich Features (~2 weeks)

- Reactions (TAGMSG `+react`, aggregate display, click to toggle)
- Replies (TAGMSG `+reply`, inline rendering, scroll-to-parent)
- Typing indicators (TAGMSG `+typing`)
- File upload (drag-and-drop, paste, AT Protocol PDS upload, progress)
- Media embeds (images, video, audio from tags)
- Link previews
- Markdown-lite rendering
- Emoji picker
- @mention and #channel autocomplete
- Message editing (if server support ready)
- Message deletion (if server support ready)

### Phase 3: Polish (~1 week)

- Unread tracking (IndexedDB)
- Notification system (browser notifications, title badge, sounds)
- Channel settings panel (modes, bans, members)
- User profile popovers
- Quick switcher (Cmd+K)
- Keyboard shortcuts
- Mobile responsive layout
- Light/dark theme
- Loading states, error handling, empty states
- Offline indicator + queued messages

### Phase 4: Production (~1 week)

- PWA manifest + service worker (offline shell, push notifications)
- Deploy to `app.freeq.at` (via Miren or static hosting)
- Automated E2E tests (Playwright)
- Performance: virtualized message list (react-window), lazy image loading
- Accessibility: ARIA labels, keyboard navigation, screen reader testing

---

## 7. Summary of Server Changes Needed

| Change | Effort | Phase | Blocks |
|--------|--------|-------|--------|
| `msgid` on all messages | Small (30 lines) | 0 | Editing, deletion, replies, reactions-to-msg, read markers |
| Message signing (`+freeq.at/sig`) | Medium (already planned) | 0 | Verified badge display |
| `away-notify` cap | Small (20 lines) | 0 | Presence indicators |
| `+typing` relay | Zero (TAGMSG works) | — | Nothing — client-only tag |
| `+reply` relay | Zero (TAGMSG works) | — | Nothing — client-only tag |
| `+react` to specific msgid | Zero (already works) | — | Already sends `+react` tag |
| Edit support (authorship + DB) | Medium (100 lines) | 0-1 | Message editing UX |
| Delete support (soft delete) | Medium (50 lines) | 0-1 | Message deletion UX |
| Search (FTS5) | Medium | 2+ | Search feature |

**Total server work for Phase 0: ~3 days.** The `msgid` and `away-notify` changes are tiny. Edit/delete is the only medium-effort item and can slip to Phase 1.

---

## 8. What We're NOT Building

- **Voice/video calls** — Out of scope. Use existing tools.
- **Custom emoji / sticker packs** — Unicode emoji only.
- **Bots marketplace** — The bot framework exists in the SDK but the web app doesn't need a bot UI.
- **Admin dashboard** — Server admin is CLI. The web app is a user tool.
- **Multi-server** — This client connects to one freeq server. Multi-network is a power-user TUI feature.
- **Offline message composition** — If disconnected, show "reconnecting" and queue. Don't pretend to be offline-first.
- **Custom themes** — Light and dark. Not a theme engine.

---

## 9. Open Questions

1. **Separate repo or monorepo?** Recommendation: separate repo (`freeq-app`). The web app has a completely different build toolchain (Node/npm vs Rust/cargo). Monorepo adds complexity with no benefit.

2. **React vs Solid vs Svelte?** React has the largest talent pool and component ecosystem. For a team tool that needs to ship fast and be maintained, React + TypeScript is the pragmatic choice.

3. **Should DMs show AT Protocol display names or IRC nicks?** Display names (from AT Protocol profile) with nick as subtitle. Users think in terms of identities, not IRC nicks.

4. **Should the app work without JavaScript?** No. This is a WebSocket-based real-time app. Server-rendered fallback would be a different product.

5. **Do we need the edit/delete protocol changes for launch?** Not strictly. Phase 1 can launch without editing. But it's a glaring gap for Slack refugees. Recommend implementing in Phase 0 alongside msgid.

6. **How do we handle the `/` command escape hatch?** Show a subtle hint in the compose box: "Type / for commands". When the user types `/`, show a command palette overlay (like Discord) with autocomplete. Never require it.
