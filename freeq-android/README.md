# freeq Android Client

Android client for the freeq IRC network, built with Kotlin and Jetpack Compose (Material 3).
Mirrors the functionality of the [iOS client](../freeq-ios) and uses the same
`freeq-sdk-ffi` library (UniFFI) for IRC connectivity and AT Protocol authentication.

## Current status

The app is fully functional with **stubbed FFI bindings** — all UI screens are implemented
and work with simulated SDK responses. To connect to a real freeq server, run
`build-rust.sh` and replace the stubs in `com.freeq.ffi` with real UniFFI-generated bindings.

### Screens implemented

- **ConnectScreen** — Bluesky OAuth login (Chrome Custom Tabs) + guest mode
- **ChatsTab** — Channel/DM list with last message preview, unread badges, search
- **ChatDetailScreen** — Full chat with message list, compose bar, member sidebar
- **MessageList** — Message grouping, date separators, system messages, reactions, reply context, typing indicators, context menu (reply/edit/delete/copy)
- **ComposeBar** — Multi-line input, @mention autocomplete, reply/edit context bars, /commands
- **DiscoverTab** — Popular channels + custom channel join
- **SettingsTab** — Account info, theme toggle, connection status, disconnect

### Architecture

```
MainActivity (entry point, deep link handler)
└── FreeqApp (theme + routing)
    ├── ConnectScreen (disconnected)
    └── MainScreen (connected)
        ├── ChatsTab → ChatDetailScreen
        ├── DiscoverTab
        └── SettingsTab
```

- **State**: `AppState` (AndroidViewModel) with Compose `mutableStateOf` / `mutableStateListOf`
- **Events**: `AndroidEventHandler` bridges FFI callbacks → `Dispatchers.Main` → state updates
- **Persistence**: SharedPreferences (nick, server, channels, read positions, theme)
- **Theme**: Material 3 dark/light with freeq accent (#6c63ff)

## Quick start

1. **Open the project** in Android Studio (Hedgehog or newer)

2. **Build & run** on emulator or device (minSdk 26):
   ```sh
   cd freeq-android
   ./gradlew assembleDebug
   ```

3. The app runs with stub FFI — connect as guest to test all UI flows.

## Building with real FFI

Run from the repo root:

```sh
./freeq-android/build-rust.sh
```

This requires `cargo-ndk` and Android NDK targets installed. See the script for details.

After running, copy the generated Kotlin sources from `Generated/` into
`freeq/src/main/java/com/freeq/ffi/` (replacing the stub files) and add JNA:

```kotlin
// freeq/build.gradle.kts
implementation("net.java.dev.jna:jna:5.13.0@aar")
```

## Future work

- Rich media rendering (images, YouTube thumbnails, Bluesky embeds)
- Message search
- User profile sheets (Bluesky profile fetch)
- Thread/reply chain view
- Photo upload with cross-post to Bluesky
- Push notifications
- Auto-reconnect with backoff
