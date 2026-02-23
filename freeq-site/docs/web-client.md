# Web Client

The freeq web client is a modern, Slack/Discord-class chat application built on standard IRC protocol. It runs at [irc.freeq.at](https://irc.freeq.at) or anywhere freeq-server is deployed.

## Features

### Identity

- **AT Protocol OAuth login** — Sign in with your Bluesky account. Your handle becomes your nick.
- **Guest mode** — Connect without authentication. Full chat access, limited features.
- **Verified badges** — Users with AT Protocol identity show a ✓ badge.
- **Rich profiles** — Click any user to see avatar, display name, bio, and DID.

### Messaging

- **Real-time messaging** — WebSocket transport with IRCv3 capabilities.
- **Message editing** — Click edit or press ↑ to edit your last message.
- **Message deletion** — Remove messages you've sent.
- **Reactions** — Emoji reactions on any message. Quick picks or full emoji picker.
- **Reply threading** — Reply to specific messages with quoted context. Thread view for following conversations.
- **Typing indicators** — See who's typing in the current channel.
- **Message history** — CHATHISTORY on join with database-backed persistence.

### Media

- **Image upload** — Button, drag-and-drop, or clipboard paste. Images stored via AT Protocol PDS.
- **Inline previews** — Images render inline. Click for fullscreen lightbox with zoom.
- **Bluesky post embeds** — Paste a `bsky.app` URL for a rich card with author, text, and images.
- **YouTube thumbnails** — YouTube links render with video thumbnail.
- **Cross-post to Bluesky** — Toggle to share uploaded images as Bluesky posts.

### Navigation

- **Channel sidebar** — Channels and DMs with unread badges.
- **Channel discovery** — Browse server channels, search, create new ones.
- **Quick switcher** — Cmd+K to jump between channels.
- **Search** — Cmd+F to search across all loaded messages.
- **DM sidebar** — Direct messages as a separate section.

### UX polish

- **Dark and light themes** — Toggle in settings.
- **Desktop notifications** — Mentions trigger browser notifications.
- **Read position** — Last-read message saved to IndexedDB per channel.
- **Member list** — Collapsible, grouped by role (ops, voiced, members).
- **Responsive** — Full mobile layout with touch-friendly actions.
- **Reconnection** — Auto-reconnect with banner on disconnect.
- **Keyboard shortcuts** — Tab autocomplete, ↑ to edit, Cmd+K quick switch.

## Running locally

```bash
cd freeq-app
npm install
npm run dev
```

Opens at `http://127.0.0.1:5173`. The Vite dev server proxies `/irc`, `/api`, and `/auth` to `127.0.0.1:8080` (the freeq server).

## Building for production

```bash
cd freeq-app
npm run build
```

Output in `freeq-app/dist/`. Serve with `--web-static-dir freeq-app/dist`.

## Desktop app (Tauri)

```bash
cd freeq-app
npx tauri build
```

Produces `.app` and `.dmg` bundles for macOS.

## Architecture

The web client is:

- **React 18** + TypeScript
- **Zustand** for state management
- **Tailwind CSS** for styling
- **Vite** for bundling
- **Pure WebSocket IRC** — no REST for chat, no custom protocol

Every feature maps to standard IRC protocol:

| Web feature | IRC protocol |
|------------|-------------|
| Messages | PRIVMSG |
| Reactions | TAGMSG `+react` |
| Editing | PRIVMSG `+draft/edit` |
| Deletion | TAGMSG `+draft/delete` |
| Replies | PRIVMSG `+reply` |
| Typing | TAGMSG `+typing` |
| History | CHATHISTORY |
| Media | PRIVMSG (URL to uploaded image) |

A standard IRC client in the same channel sees all messages. It won't see the rich rendering, but messages are never hidden or protocol-incompatible.
