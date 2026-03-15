# macOS App — Gap Analysis vs Web & iOS Clients

## Status: 58/60 items complete ✅

## Critical Bugs (P0)

1. ✅ **Members race fix** — `getOrCreateChannel` before flushing pendingNames
2. ✅ **Detail panel visibility** — Toggle button in TopBar with visual state
3. ✅ **CHATHISTORY batch case mismatch** — Case-insensitive channel/DM lookup in batchEnd
4. ✅ **Echo-message** — SDK handles echo-message cap; app relies on server echo (correct)

---

## Product Gaps (vs Web App)

### Identity & Profiles
5. ✅ **Bluesky avatars** — ProfileCache + AvatarView with disk caching
6. ✅ **Display names** — From Bluesky profile, shown in message headers + member list
7. ✅ **Verified badges** — ✓ checkmark.seal.fill for AT-authenticated users
8. ✅ **DID resolution** — WHOIS 330 parsing, rate-limited drain queue
9. ✅ **User profile sheet** — Full profile: avatar, name, handle, DID, bio, stats, shared channels

### Messages
10. ✅ **Link previews** — OG proxy unfurling with title/desc/image
11. ✅ **Bluesky post embeds** — Rich cards with author avatar, text, click to open
12. ✅ **Image lightbox** — Click image → popover with full size, Copy/Open/Save
13. ✅ **Signed message badge** — 🔒 green lock on `+freeq.at/sig` messages (FFI `is_signed`)
14. ✅ **E2EE badge** — Lock+shield badge on encrypted DMs, E2eeManager wraps FreeqE2ee FFI
15. ✅ **Full timestamps on hover** — `.help()` tooltip with "Monday, Mar 2, 2026 at 13:45:23"
16. ✅ **Message grouping** — Consecutive same-sender messages collapsed, compact mode available

### Compose
17. ✅ **Emoji picker** — System Character Viewer (😀 button + ⌘⌃Space)
18. ✅ **Slash command autocomplete** — Popup dropdown with 17 commands + descriptions
19. ✅ **@mention autocomplete** — Popup when typing @nick, shows channel members
20. ✅ **:emoji: autocomplete** — Popup when typing :keyword, 32 common emoji
21. ✅ **File/image upload** — Drag-drop, paste, file picker, upload preview, progress
22. ✅ **Format toolbar** — Bold/Italic/Code/Strikethrough/Link buttons
23. ✅ **Input history** — Sent messages tracked for cycling
24. ✅ **Cross-post toggle** — Cloud button for Bluesky cross-posting (persisted)

### Sidebar
25. ✅ **Favorites** — Right-click → Favorite, pinned at top of sidebar, persisted
26. ✅ **Muted channels** — Right-click → Mute, dimmed, no notifications, persisted
27. ✅ **Last message preview** — Shown in sidebar for channels + DMs
28. ✅ **Sidebar sorting** — DMs sorted by last activity, channels alphabetical
29. ✅ **Channel list browser** — ⇧⌘L opens browser, search/filter, click to join

### Right Panel
30. ✅ **Channel settings** — Click member count → topic edit, ops list, PINS, leave
31. ✅ **Pinned messages bar** — Orange bar, expand all pins, click to scroll, pin/unpin from context menu

### Navigation
32. ✅ **Search (⌘F)** — Message search in current channel, click result → scroll
33. ✅ **Scroll-to-message** — From search, reply click, bookmark, pin click
34. ✅ **Thread view** — Side panel with root message, replies, quick reply bar
35. ✅ **Bookmarks** — Right-click → Bookmark, ⇧⌘B panel, click to jump, persisted

### Connection
36. ✅ **Server MOTD** — Collapsible banner, parsed from SDK ServerNotice
37. ✅ **Guest upgrade banner** — Blue banner for unauthenticated users
38. ✅ **Onboarding tour** — First-launch welcome with feature highlights + shortcuts

---

## Product Gaps (vs iOS App)

39. ✅ **Avatar cache** — Memory + disk two-tier caching (~/Library/Caches/at.freeq.macos/avatars/)
40. ✅ **User profile sheet** — Full profile with shared channels, Bluesky link
41. ✅ **Sound effects** — Ping/Tink/Pop/Basso for mention/DM/connect/disconnect (toggle in Settings)
42. ✅ **Background reconnect** — NSWorkspace didWake notification → auto-reconnect
43. ✅ **Dock badge** — Total unread count on app icon

---

## Technical Gaps

### SDK Integration
44. ✅ **WHOIS events in FFI** — Dedicated WhoisReply event with nick + info
45. 🔲 **Account-notify** — SDK doesn't handle ACCOUNT command yet; WHOIS workaround sufficient
46. 🔲 **Extended-join** — Would require SDK Event::Joined change (breaking); WHOIS workaround sufficient
47. ✅ **Message signing** — SDK auto-generates ed25519 keypair + MSGSIG after SASL success
48. ✅ **E2EE** — E2eeManager wraps FreeqE2ee, Settings UI for init/enable, session tracking
49. ✅ **MSGSIG registration** — SDK handles automatically after auth
50. ✅ **Caps negotiation** — SDK requests echo-message, message-tags, etc.

### Data Persistence
51. ✅ **Local message database** — SQLite WAL mode, store/load/search/edit/delete
52. ✅ **Read position tracking** — lastReadMsgId per channel, updated on focus
53. ✅ **Settings persistence** — @AppStorage for compact mode, join/part, sounds, notifications

### Performance
54. ✅ **Message virtualization** — LazyVStack handles this natively
55. ✅ **Avatar disk caching** — ~/Library/Caches/at.freeq.macos/avatars/
56. ✅ **Image proxy** — Uses server OG proxy (https://irc.freeq.at/api/v1/og)

### Architecture
57. ✅ **Error recovery** — Key FFI calls wrapped in do/catch with logging
58. ✅ **Structured logging** — os.Logger with subsystems: irc, auth, p2p, ui, media, profile
59. ✅ **showJoinPart wired** — @AppStorage controls system message visibility
60. ✅ **compactMode wired** — Inline timestamp+nick layout, no avatars/grouping

---

## Remaining (2 items — SDK limitations, not app issues)

| # | Item | Status | Notes |
|---|------|--------|-------|
| 45 | Account-notify | 🔲 | SDK doesn't handle ACCOUNT command; WHOIS workaround sufficient |
| 46 | Extended-join | 🔲 | Would require breaking SDK Event::Joined change; WHOIS workaround |
