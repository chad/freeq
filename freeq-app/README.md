# freeq web client

The browser-based client for [freeq](https://github.com/chad/freeq) — IRC with
AT Protocol identity.

**Live at [irc.freeq.at](https://irc.freeq.at)**

## Features

- **AT Protocol OAuth** — sign in with your Bluesky identity
- **Cryptographic message signing** — per-session ed25519 keys, verified badge (✓)
- **End-to-end encrypted DMs** — X3DH key agreement + Double Ratchet
- **Channel encryption** — passphrase-based AES-256-GCM
- **Message editing & deletion** — edit with ↑, delete from context menu
- **Reactions & threads** — emoji reactions, threaded replies
- **Bluesky cross-posting** — optionally post messages to Bluesky
- **Pinned messages** — pin important messages, view in sidebar
- **Rich embeds** — images, video, audio, Bluesky posts, YouTube, link previews
- **Media upload** — drag-and-drop or paste images/files (stored on your PDS)
- **Channel policies** — gate access with verifiable credentials (GitHub, Bluesky follows)
- **Shareable invite links** — `https://irc.freeq.at/join/#channel`
- **Bookmarks** — save messages for later (⌘B)
- **Search** — full-text message search (⌘F)
- **CHATHISTORY** — scroll back to load older messages on demand
- **Guest mode** — connect without authentication, standard IRC
- **PWA installable** — add to home screen on mobile
- **Dark theme** — designed for dark mode

## Setup

```bash
cd freeq-app
npm install
npm run dev
```

The dev server runs at `http://localhost:5173` and proxies WebSocket
connections to a local freeq-server at `ws://localhost:8080/irc`.

### Build for production

```bash
npm run build
```

Output goes to `dist/`. Serve it with any static file server, or point
the freeq-server at it:

```bash
freeq-server --web-static-dir freeq-app/dist --web-addr 0.0.0.0:8080
```

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `⌘K` | Quick channel switcher |
| `⌘F` | Search messages |
| `⌘B` | Bookmarks panel |
| `⌘/` | Keyboard shortcuts help |
| `⌥1`–`⌥0` | Switch to channel by position |
| `⌥↑` / `⌥↓` | Previous / next channel |
| `↑` (empty input) | Edit last message |
| `Esc` | Close modals and panels |

## Stack

- **React 19** + TypeScript
- **Vite** build tooling
- **Tailwind CSS** styling
- **Zustand** state management
- **Native WebSocket** IRC transport

## Project Structure

```
src/
  App.tsx              Main layout + routing
  store.ts             Zustand store (channels, messages, state)
  irc/
    client.ts          WebSocket IRC client + message parser
    parser.ts          IRC protocol parser
  components/
    ConnectScreen.tsx   Login / connect page
    Sidebar.tsx         Channel list + DM list
    MessageList.tsx     Message display + embeds + history
    ComposeBox.tsx      Input box + slash commands + file upload
    TopBar.tsx          Channel header + member toggle
    MemberList.tsx      Channel member sidebar
    ...
  hooks/
    useKeyboard.ts     Global keyboard shortcuts
  lib/
    profiles.ts        AT Protocol profile fetching + caching
    e2ee.ts            E2EE key management
```
