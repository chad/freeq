import SwiftUI

struct ComposeBar: View {
    @Environment(AppState.self) private var appState
    @State private var text: String = ""
    @FocusState private var isFocused: Bool
    @State private var pendingUpload: PendingUpload?
    @State private var isUploading = false
    @State private var autocompleteIndex: Int = 0
    @AppStorage("freeq.crossPostBluesky") private var crossPostBluesky = false

    private var isEditing: Bool { appState.editingMessageId != nil }
    private var isReplying: Bool { appState.replyingToMessage != nil }

    var body: some View {
        VStack(spacing: 0) {
            // Reply/Edit banner
            if let reply = appState.replyingToMessage {
                HStack(spacing: 6) {
                    Image(systemName: "arrowshape.turn.up.left.fill")
                        .font(.caption)
                        .foregroundColor(.accentColor)
                    Text("Replying to **\(reply.from)**")
                        .font(.caption)
                    Text(reply.text)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer()
                    Button { appState.replyingToMessage = nil } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .background(Color.accentColor.opacity(0.05))
            }

            if isEditing {
                HStack(spacing: 6) {
                    Image(systemName: "pencil")
                        .font(.caption)
                        .foregroundStyle(.orange)
                    Text("Editing message")
                        .font(.caption)
                        .foregroundStyle(.orange)
                    Spacer()
                    Button {
                        appState.editingMessageId = nil
                        appState.editingText = nil
                        text = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .background(Color.orange.opacity(0.05))
            }

            // Upload preview
            if let upload = pendingUpload {
                HStack(spacing: 8) {
                    if let preview = upload.preview {
                        Image(nsImage: preview)
                            .resizable()
                            .aspectRatio(contentMode: .fit)
                            .frame(width: 60, height: 60)
                            .clipShape(RoundedRectangle(cornerRadius: 6))
                    } else {
                        Image(systemName: "doc.fill")
                            .font(.title2)
                            .frame(width: 60, height: 60)
                    }
                    VStack(alignment: .leading, spacing: 2) {
                        Text(upload.filename)
                            .font(.caption.weight(.medium))
                            .lineLimit(1)
                        if isUploading {
                            ProgressView()
                                .scaleEffect(0.6)
                        }
                        if let error = upload.error {
                            Text(error)
                                .font(.caption2)
                                .foregroundStyle(.red)
                        }
                    }
                    Spacer()
                    Button { pendingUpload = nil } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .background(Color(nsColor: .controlBackgroundColor).opacity(0.5))
            }

            // Autocomplete popup
            AutocompletePopup(text: $text, selectedIndex: $autocompleteIndex, anchor: .zero)

            HStack(alignment: .bottom, spacing: 6) {
                // File picker button
                Button { pickFile() } label: {
                    Image(systemName: "plus.circle")
                        .font(.title3)
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Attach file (images)")
                .disabled(appState.authenticatedDID == nil)

                // Text editor
                ZStack(alignment: .topLeading) {
                    if text.isEmpty {
                        Text("Message \(appState.activeChannel ?? "")…")
                            .foregroundStyle(.tertiary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 8)
                    }
                    ComposeTextView(
                        text: $text,
                        onSubmit: send,
                        onUpArrow: editLastMessage,
                        members: appState.activeChannelState?.members.map(\.nick) ?? []
                    )
                    .frame(minHeight: 32, maxHeight: 120)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .padding(4)
                .background(
                    RoundedRectangle(cornerRadius: 8)
                        .fill(Color(nsColor: .controlBackgroundColor))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 8)
                        .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
                )

                // Format toolbar
                FormatToolbar(text: $text)

                // Emoji button (system picker)
                Button {
                    NSApp.orderFrontCharacterPalette(nil)
                } label: {
                    Image(systemName: "face.smiling")
                        .font(.title3)
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Emoji (⌘⌃Space)")

                // Cross-post toggle
                if appState.authenticatedDID != nil {
                    Button {
                        crossPostBluesky.toggle()
                    } label: {
                        Image(systemName: crossPostBluesky ? "cloud.fill" : "cloud")
                            .font(.caption)
                            .foregroundStyle(crossPostBluesky ? .blue : .secondary)
                    }
                    .buttonStyle(.plain)
                    .help(crossPostBluesky ? "Cross-posting to Bluesky (click to disable)" : "Cross-post to Bluesky")
                }

                // Send button
                Button { send() } label: {
                    Image(systemName: isEditing ? "checkmark.circle.fill" : "arrow.up.circle.fill")
                        .font(.title2)
                        .symbolRenderingMode(.hierarchical)
                        .foregroundColor(text.isEmpty && pendingUpload == nil ? .gray : (isEditing ? .orange : .accentColor))
                }
                .buttonStyle(.plain)
                .disabled(text.isEmpty && pendingUpload == nil)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
        .background(.bar)
        .onDrop(of: [.image, .fileURL], isTargeted: nil) { providers in
            handleDrop(providers)
            return true
        }
        .onChange(of: text) { _, newValue in
            if !newValue.isEmpty, let target = appState.activeChannel {
                appState.sendTyping(target: target)
            }
        }
        .onChange(of: appState.editingText) { _, newValue in
            if let newValue {
                text = newValue
            }
        }
    }

    // Input history
    @State private var history: [String] = []
    @State private var historyIndex: Int = -1

    private func send() {
        // Handle pending upload
        if pendingUpload != nil {
            uploadAndSend()
            return
        }

        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let target = appState.activeChannel else { return }

        // Save to history
        if !trimmed.hasPrefix("/") || trimmed.hasPrefix("/me ") {
            history.append(trimmed)
            if history.count > 100 { history.removeFirst() }
        }
        historyIndex = -1

        // Editing mode
        if let editId = appState.editingMessageId {
            appState.editMessage(target: target, msgId: editId, newText: trimmed)
            appState.editingMessageId = nil
            appState.editingText = nil
            text = ""
            return
        }

        // Handle slash commands
        if trimmed.hasPrefix("/") {
            handleCommand(trimmed, target: target)
            text = ""
            return
        }

        // Normal messages (split multi-line)
        let replyId = appState.replyingToMessage?.id
        for line in trimmed.components(separatedBy: .newlines) {
            let l = line.trimmingCharacters(in: .whitespaces)
            if !l.isEmpty {
                if let replyId {
                    appState.sendRaw("@+reply=\(replyId) PRIVMSG \(target) :\(l)")
                    appState.replyingToMessage = nil
                } else {
                    appState.sendMessage(to: target, text: l)
                }
            }
        }
        text = ""
    }

    private func editLastMessage() {
        guard text.isEmpty, let target = appState.activeChannel else { return }
        if let lastMsg = appState.lastOwnMessage(in: target) {
            appState.editingMessageId = lastMsg.id
            appState.editingText = lastMsg.text
            text = lastMsg.text
        }
    }

    // MARK: - File Upload

    private func pickFile() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image, .png, .jpeg, .gif]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        if panel.runModal() == .OK, let url = panel.url {
            loadFile(url: url)
        }
    }

    private func handleDrop(_ providers: [NSItemProvider]) {
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier("public.file-url") {
                provider.loadItem(forTypeIdentifier: "public.file-url") { item, _ in
                    if let data = item as? Data, let url = URL(dataRepresentation: data, relativeTo: nil) {
                        DispatchQueue.main.async { loadFile(url: url) }
                    }
                }
            } else if provider.canLoadObject(ofClass: NSImage.self) {
                provider.loadObject(ofClass: NSImage.self) { item, _ in
                    if let image = item as? NSImage {
                        DispatchQueue.main.async {
                            if let tiff = image.tiffRepresentation,
                               let rep = NSBitmapImageRep(data: tiff),
                               let png = rep.representation(using: .png, properties: [:]) {
                                pendingUpload = PendingUpload(
                                    data: png, filename: "paste.png",
                                    contentType: "image/png", preview: image
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    private func loadFile(url: URL) {
        guard let data = try? Data(contentsOf: url) else { return }
        let filename = url.lastPathComponent
        let ext = url.pathExtension.lowercased()
        let contentType: String
        switch ext {
        case "png": contentType = "image/png"
        case "jpg", "jpeg": contentType = "image/jpeg"
        case "gif": contentType = "image/gif"
        case "webp": contentType = "image/webp"
        default: contentType = "application/octet-stream"
        }
        let preview = NSImage(contentsOf: url)
        pendingUpload = PendingUpload(data: data, filename: filename, contentType: contentType, preview: preview)
    }

    private func uploadAndSend() {
        guard let upload = pendingUpload,
              let did = appState.authenticatedDID,
              let target = appState.activeChannel else { return }

        isUploading = true
        Task {
            do {
                let url = try await FileUploader.upload(
                    data: upload.data,
                    filename: upload.filename,
                    contentType: upload.contentType,
                    did: did,
                    channel: target.hasPrefix("#") ? target : nil
                )
                let msgText = text.trimmingCharacters(in: .whitespacesAndNewlines)
                let finalText = msgText.isEmpty ? url : "\(msgText) \(url)"
                await MainActor.run {
                    appState.sendMessage(to: target, text: finalText)
                    pendingUpload = nil
                    isUploading = false
                    text = ""
                }
            } catch {
                await MainActor.run {
                    pendingUpload?.error = error.localizedDescription
                    isUploading = false
                }
            }
        }
    }

    private func handleCommand(_ input: String, target: String) {
        let parts = input.dropFirst().split(separator: " ", maxSplits: 1)
        let cmd = parts.first.map(String.init)?.lowercased() ?? ""
        let arg = parts.count > 1 ? String(parts[1]) : ""

        switch cmd {
        case "join", "j":
            arg.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
                .forEach { appState.joinChannel(String($0)) }
        case "part", "leave":
            appState.partChannel(arg.isEmpty ? target : arg)
        case "topic", "t":
            if !arg.isEmpty { appState.sendRaw("TOPIC \(target) :\(arg)") }
        case "nick":
            if !arg.isEmpty { appState.sendRaw("NICK \(arg)") }
        case "me", "action":
            if !arg.isEmpty { appState.sendAction(to: target, text: arg) }
        case "msg", "query":
            let mp = arg.split(separator: " ", maxSplits: 1)
            if mp.count == 2 {
                let dmTarget = String(mp[0])
                appState.sendMessage(to: dmTarget, text: String(mp[1]))
                let dm = appState.getOrCreateDM(dmTarget)
                appState.activeChannel = dm.name
            }
        case "kick", "k":
            let kp = arg.split(separator: " ", maxSplits: 1)
            if let user = kp.first {
                appState.kickUser(target, String(user), reason: kp.count > 1 ? String(kp[1]) : nil)
            }
        case "op":
            if !arg.isEmpty { appState.setMode(target, "+o", arg) }
        case "deop":
            if !arg.isEmpty { appState.setMode(target, "-o", arg) }
        case "voice":
            if !arg.isEmpty { appState.setMode(target, "+v", arg) }
        case "invite":
            if !arg.isEmpty { appState.inviteUser(target, arg) }
        case "away":
            appState.setAway(arg.isEmpty ? nil : arg)
        case "whois", "wi":
            if !arg.isEmpty { appState.sendWhois(arg) }
        case "mode", "m":
            if !arg.isEmpty {
                appState.sendRaw("MODE \(arg.hasPrefix("#") ? "" : "\(target) ")\(arg)")
            }
        case "raw", "quote":
            appState.sendRaw(arg)
        case "p2p":
            handleP2pCommand(arg)
        case "help":
            let ch = appState.activeChannelState
            let help = [
                "── Commands ──",
                "/join #channel · /part · /topic text",
                "/kick user · /op user · /voice user · /invite user",
                "/whois user · /away reason · /me action",
                "/msg user text · /mode +o user · /raw IRC_LINE",
                "/p2p start|id|connect|peers",
                "── Shortcuts ──",
                "⌘K quick switch · ⌘J join · ↑ edit last · Esc cancel edit"
            ]
            for line in help {
                ch?.appendIfNew(ChatMessage(
                    id: UUID().uuidString, from: "system", text: line,
                    isAction: false, timestamp: Date(), replyTo: nil
                ))
            }
        default:
            appState.sendRaw("\(cmd.uppercased())\(arg.isEmpty ? "" : " \(arg)")")
        }
    }

    private func handleP2pCommand(_ arg: String) {
        let parts = arg.split(separator: " ", maxSplits: 1)
        let subcmd = parts.first.map(String.init) ?? ""
        let ch = appState.activeChannelState

        switch subcmd {
        case "start":
            appState.startP2p()
            ch?.appendIfNew(ChatMessage(id: UUID().uuidString, from: "system",
                text: "P2P subsystem starting…", isAction: false, timestamp: Date(), replyTo: nil))
        case "id":
            if let id = appState.p2pEndpointId {
                ch?.appendIfNew(ChatMessage(id: UUID().uuidString, from: "system",
                    text: "Your iroh endpoint: \(id)", isAction: false, timestamp: Date(), replyTo: nil))
            } else {
                ch?.appendIfNew(ChatMessage(id: UUID().uuidString, from: "system",
                    text: "P2P not active. Use /p2p start", isAction: false, timestamp: Date(), replyTo: nil))
            }
        case "connect":
            if parts.count > 1 {
                appState.connectP2pPeer(String(parts[1]))
            }
        case "peers":
            let peers = appState.p2pConnectedPeers
            let msg = peers.isEmpty ? "No P2P peers connected" : "P2P peers: \(peers.joined(separator: ", "))"
            ch?.appendIfNew(ChatMessage(id: UUID().uuidString, from: "system",
                text: msg, isAction: false, timestamp: Date(), replyTo: nil))
        default:
            ch?.appendIfNew(ChatMessage(id: UUID().uuidString, from: "system",
                text: "P2P commands: start, id, connect <endpoint>, peers", isAction: false, timestamp: Date(), replyTo: nil))
        }
    }
}

/// NSTextView wrapper that handles Enter vs Shift+Enter, Up arrow, and Tab completion.
struct ComposeTextView: NSViewRepresentable {
    @Binding var text: String
    var onSubmit: () -> Void
    var onUpArrow: () -> Void
    var members: [String]  // For tab completion

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = NSScrollView()
        let textView = ComposeNSTextView()
        textView.delegate = context.coordinator
        textView.isRichText = false
        textView.font = .systemFont(ofSize: NSFont.systemFontSize)
        textView.allowsUndo = true
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: 4, height: 6)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.textContainer?.widthTracksTextView = true
        textView.submitAction = onSubmit
        textView.upArrowAction = onUpArrow

        scrollView.documentView = textView
        scrollView.hasVerticalScroller = false
        scrollView.drawsBackground = false
        scrollView.borderType = .noBorder

        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? ComposeNSTextView else { return }
        if textView.string != text {
            textView.string = text
        }
        textView.submitAction = onSubmit
        textView.upArrowAction = onUpArrow
        textView.members = members
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    class Coordinator: NSObject, NSTextViewDelegate {
        let parent: ComposeTextView

        init(parent: ComposeTextView) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? NSTextView else { return }
            parent.text = textView.string
        }
    }
}

class ComposeNSTextView: NSTextView {
    var submitAction: (() -> Void)?
    var upArrowAction: (() -> Void)?
    var members: [String] = []
    private var tabCompletionCandidates: [String] = []
    private var tabCompletionIndex: Int = 0
    private var tabCompletionPrefix: String = ""

    override func keyDown(with event: NSEvent) {
        // Enter without Shift = send
        if event.keyCode == 36 && !event.modifierFlags.contains(.shift) {
            resetTabCompletion()
            submitAction?()
            return
        }
        // Up arrow when text is empty = edit last
        if event.keyCode == 126 && string.isEmpty {
            upArrowAction?()
            return
        }
        // Escape = cancel edit
        if event.keyCode == 53 {
            string = ""
            resetTabCompletion()
            NotificationCenter.default.post(name: .cancelEdit, object: nil)
            return
        }
        // Tab = nick completion
        if event.keyCode == 48 {
            performTabCompletion()
            return
        }
        // Any other key resets tab completion
        if event.keyCode != 48 {
            resetTabCompletion()
        }
        super.keyDown(with: event)
    }

    private func performTabCompletion() {
        if tabCompletionCandidates.isEmpty {
            // Start new completion
            let text = string
            guard let lastWord = text.split(separator: " ").last else { return }
            let prefix = String(lastWord).lowercased()
            let candidates = members.filter { $0.lowercased().hasPrefix(prefix) }.sorted()
            guard !candidates.isEmpty else { return }

            tabCompletionPrefix = prefix
            tabCompletionCandidates = candidates
            tabCompletionIndex = 0
        } else {
            // Cycle through candidates
            tabCompletionIndex = (tabCompletionIndex + 1) % tabCompletionCandidates.count
        }

        // Replace the prefix with the candidate
        let candidate = tabCompletionCandidates[tabCompletionIndex]
        var text = string
        // Find and replace the last word
        if let range = text.range(of: tabCompletionPrefix, options: [.backwards, .caseInsensitive]) {
            let isStartOfLine = range.lowerBound == text.startIndex ||
                text[text.index(before: range.lowerBound)] == " "
            let suffix = isStartOfLine && text.distance(from: text.startIndex, to: range.lowerBound) == 0 ? ": " : " "
            text.replaceSubrange(range, with: candidate + suffix)
        } else if let prevCandidate = tabCompletionCandidates[safe: tabCompletionIndex == 0 ? tabCompletionCandidates.count - 1 : tabCompletionIndex - 1] {
            // Replace previous candidate
            let suffixes = [": ", " "]
            for suf in suffixes {
                if let range = text.range(of: prevCandidate + suf, options: [.backwards, .caseInsensitive]) {
                    let isStart = range.lowerBound == text.startIndex
                    let newSuf = isStart ? ": " : " "
                    text.replaceSubrange(range, with: candidate + newSuf)
                    break
                }
            }
        }
        string = text
        // Move cursor to end
        setSelectedRange(NSRange(location: string.count, length: 0))
        // Notify delegate of change
        delegate?.textDidChange?(Notification(name: NSText.didChangeNotification, object: self))
    }

    private func resetTabCompletion() {
        tabCompletionCandidates = []
        tabCompletionIndex = 0
        tabCompletionPrefix = ""
    }
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

extension Notification.Name {
    static let cancelEdit = Notification.Name("cancelEdit")
}
