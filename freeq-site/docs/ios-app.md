# iOS App

The freeq iOS app is a native SwiftUI application with full feature parity to the web client. It uses the Rust SDK via UniFFI for all IRC communication.

## Features

- **Bluesky login** — AT Protocol OAuth, identical to the web client.
- **WhatsApp/Telegram-style navigation** — Tab bar with Chats, Discover, Settings.
- **Full message support** — Reactions (long-press), replies (swipe), editing, deletion.
- **Inline images** — With pinch-to-zoom lightbox and share button.
- **Bluesky embeds** — Rich cards for `bsky.app` links.
- **Photo upload** — Camera or photo library, with Bluesky cross-post toggle.
- **Chat history** — `CHATHISTORY LATEST` on join, "Load older messages" button.
- **Typing indicators** — Send and display.
- **Local notifications** — DMs and mentions trigger notifications.
- **Network reconnect** — `NWPathMonitor` detects network changes, auto-reconnects.
- **Haptic feedback** — On send, react, delete, and swipe.
- **Dark and light themes** — Follows system setting or manual toggle.
- **Unread tracking** — Per-channel badges in the sidebar.
- **Nick autocomplete** — Type `@` to see channel members.
- **Thread viewer** — Tap a reply to see the full reply chain.
- **User profiles** — Tap avatar to see full profile with DID and Bluesky link.
- **Verified badges** — Users with AT Protocol identity show ✓.

## Building

### Prerequisites

- Xcode 15+
- Rust toolchain with iOS targets:
  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim
  ```
- [xcodegen](https://github.com/yonaskolb/XcodeGen): `brew install xcodegen`

### Build steps

```bash
# Build Rust SDK for iOS + generate Swift bindings
./freeq-ios/build-rust.sh

# Generate Xcode project
cd freeq-ios && xcodegen generate

# Open in Xcode
open freeq.xcodeproj
```

Build and run on device or simulator from Xcode.

### Signing

Set your development team in `freeq-ios/project.yml`:

```yaml
settings:
  DEVELOPMENT_TEAM: YOUR_TEAM_ID
```

## Architecture

```
SwiftUI Views
     ↓
AppState (ObservableObject)
     ↓
FFI Bridge (freeq_sdk_ffi)
     ↓
Rust SDK (freeq_sdk)
     ↓
IRC over TCP
```

The FFI bridge uses UniFFI with a UDL interface definition (`freeq-sdk-ffi/src/freeq.udl`). The Rust SDK runs on a tokio runtime in a static `Lazy<Runtime>` with 2 worker threads.

### Key files

| Path | Description |
|------|-------------|
| `freeq-ios/freeq/Models/AppState.swift` | Central state, FFI calls, event loop |
| `freeq-ios/freeq/Views/ChatsTab.swift` | Channel list with previews |
| `freeq-ios/freeq/Views/ChatDetailView.swift` | Message view + compose |
| `freeq-ios/freeq/Views/ConnectView.swift` | Login screen |
| `freeq-ios/freeq/Theme.swift` | Color palette |
| `freeq-sdk-ffi/src/lib.rs` | Rust FFI implementation |
| `freeq-sdk-ffi/src/freeq.udl` | UniFFI interface definition |

### Crypto note

The iOS build uses `ring` instead of `aws-lc-rs` for TLS. This is because `aws-lc-rs` references `__chkstk_darwin` which doesn't exist on iOS. The `ring` feature is enabled in the FFI crate:

```toml
[dependencies]
freeq-sdk = { path = "../freeq-sdk", default-features = false, features = ["ring"] }
```
