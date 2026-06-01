import AVFoundation
import Foundation
import Security

// MARK: - AV server endpoints

extension AppState {
    /// MoQ SFU base URL — the dedicated QUIC/WebTransport listener on :8080,
    /// the same endpoint the web client and iOS use. NOT the :443 reverse
    /// proxy, which serves an older, audio-starved WebSocket MoQ.
    var sfuBaseUrl: String {
        "https://\(avHost):8080"
    }

    /// REST API base for session discovery.
    var avApiBaseUrl: String {
        "https://\(avHost)"
    }

    /// Host portion of `serverAddress`, stripped of any `:port` suffix.
    private var avHost: String {
        serverAddress.split(separator: ":").first.map(String.init) ?? "irc.freeq.at"
    }
}

// MARK: - Call lifecycle

extension AppState {
    /// Outcome of a REST probe for an active session on a channel.
    enum ActiveSessionProbe {
        case found(sessionId: String)
        case none
    }

    /// 8-char lowercase hex per-device instance id.
    static func generateAvInstanceId() -> String {
        var bytes = [UInt8](repeating: 0, count: 4)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return bytes.map { String(format: "%02x", $0) }.joined()
    }

    /// Start or join a voice session on a channel. Always resolves the
    /// channel's *live* session from the server before joining — the
    /// in-memory `activeAvSessions` cache can point at a dead session.
    func startOrJoinVoice(channel: String) {
        guard !isInCall else { return }
        Task { await discoverAndJoinOrStart(channel: channel) }
    }

    /// `/av leave` / `/av end` entrypoint used by the UI and slash command.
    func toggleCameraEnabled() { toggleCamera() }

    private func discoverAndJoinOrStart(channel: String) async {
        let key = channel.lowercased()
        let encoded = channel.addingPercentEncoding(withAllowedCharacters: .urlHostAllowed) ?? channel
        let url = URL(string: "\(avApiBaseUrl)/api/v1/channels/\(encoded)/sessions")

        if let url {
            var req = URLRequest(url: url)
            req.timeoutInterval = 4
            if let (data, _) = try? await URLSession.shared.data(for: req),
               let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                if let active = json["active"] as? [String: Any],
                   let sessionState = active["state"] as? String,
                   sessionState == "Active",
                   let sessionId = active["id"] as? String {
                    await MainActor.run {
                        self.activeAvSessions[key] = sessionId
                        self.startCall(channel: channel, sessionId: sessionId)
                    }
                } else {
                    await MainActor.run {
                        self.activeAvSessions.removeValue(forKey: key)
                        self.startFreshAvSession(channel: channel)
                    }
                }
                return
            }
        }

        await MainActor.run {
            if let cached = self.activeAvSessions[key] {
                self.startCall(channel: channel, sessionId: cached)
            } else {
                self.startFreshAvSession(channel: channel)
            }
        }
    }

    /// Mint a per-device instance id, mark the channel pending, and put
    /// `av-start` on the wire. We join once the server echoes `started`.
    func startFreshAvSession(channel: String) {
        let instance = Self.generateAvInstanceId()
        currentAvInstance = instance
        pendingAvStart.insert(channel.lowercased())
        sendRaw("@+freeq.at/av-start;+freeq.at/av-instance=\(instance) TAGMSG \(channel)")
    }

    /// Construct the MoQ session, start mic capture, and announce the join.
    func startCall(channel: String, sessionId: String) {
        let instance = currentAvInstance ?? Self.generateAvInstanceId()
        currentAvInstance = instance
        do {
            let handler = AvCallbackHandler(appState: self)
            avSession = try FreeqAv(
                serverUrl: sfuBaseUrl,
                sessionId: sessionId,
                nick: nick,
                instanceId: instance,
                handler: handler
            )
            startLocalMic()
            sendRaw("@+freeq.at/av-join;+freeq.at/av-id=\(sessionId);+freeq.at/av-instance=\(instance) TAGMSG \(channel)")
            isInCall = true
            currentCallChannel = channel
            currentCallSessionId = sessionId
        } catch {
            print("[av] Failed to start call: \(error)")
            currentAvInstance = nil
        }
    }

    func leaveCall() {
        if let channel = currentCallChannel, let sessionId = currentCallSessionId {
            let instanceTag = currentAvInstance.map { ";+freeq.at/av-instance=\($0)" } ?? ""
            sendRaw("@+freeq.at/av-leave;+freeq.at/av-id=\(sessionId)\(instanceTag) TAGMSG \(channel)")
        }
        teardownLocal()
    }

    /// Tear down the call without sending `av-leave` (the wire is gone).
    func tearDownCallLocallyOnDisconnect() {
        teardownLocal()
    }

    private func teardownLocal() {
        cameraCapture?.stop()
        cameraCapture = nil
        micCapture?.stop()
        micCapture = nil
        avSession?.leave()
        avSession = nil
        currentAvInstance = nil
        isInCall = false
        isMuted = false
        isCameraOn = false
        isCallExpanded = false
        callParticipants = []
        participantsWithVideo = []
        currentCallChannel = nil
        currentCallSessionId = nil
    }

    func toggleMute() {
        isMuted.toggle()
        avSession?.setMuted(muted: isMuted)
    }

    func toggleCamera() {
        let next = !isCameraOn
        if next { startLocalCamera() } else { stopLocalCamera() }
        isCameraOn = next
    }

    private func startLocalMic() {
        guard avSession != nil else { return }
        let cap = CallMicCapture()
        cap.onSamples = { [weak self] samples in
            self?.avSession?.pushAudioFrame(samples: samples)
        }
        micCapture = cap
        cap.start()
    }

    private func startLocalCamera() {
        guard let av = avSession else { return }
        if cameraCapture == nil {
            let cap = CallCameraCapture()
            cap.onFrame = { [weak self] ptr, length, width, height, ts in
                guard let av = self?.avSession else { return }
                let bytes = Array(UnsafeBufferPointer(start: ptr, count: length))
                av.pushVideoFrame(bgra: bytes, width: UInt32(width), height: UInt32(height), timestampUs: ts)
            }
            cameraCapture = cap
        }
        do {
            try av.setCameraEnabled(enabled: true)
        } catch {
            print("[av] setCameraEnabled(true) failed: \(error)")
            return
        }
        cameraCapture?.start()
    }

    private func stopLocalCamera() {
        cameraCapture?.stop()
        do {
            try avSession?.setCameraEnabled(enabled: false)
        } catch {
            print("[av] setCameraEnabled(false): \(error)")
        }
    }

    /// Called by `RemoteVideoTile`. Weakly retains the display layer.
    func bindVideoSink(nick: String, to layer: AVSampleBufferDisplayLayer) {
        remoteVideoLayers.setObject(layer, forKey: nick.lowercased() as NSString)
    }

    func videoLayer(for nick: String) -> AVSampleBufferDisplayLayer? {
        remoteVideoLayers.object(forKey: nick.lowercased() as NSString)
    }

    /// Handle an inbound `+freeq.at/av-state` TAGMSG.
    func handleAvState(_ avState: String, sessionId: String, actor: String, channel: String) {
        let chanKey = channel.lowercased()
        let inThisCall = isInCall && currentCallChannel?.lowercased() == chanKey
        switch avState {
        case "started":
            activeAvSessions[chanKey] = sessionId
            if pendingAvStart.contains(chanKey) && actor.lowercased() == nick.lowercased() {
                pendingAvStart.remove(chanKey)
                startCall(channel: channel, sessionId: sessionId)
            }
        case "ended":
            activeAvSessions.removeValue(forKey: chanKey)
            pendingAvStart.remove(chanKey)
            if inThisCall { tearDownCallLocallyOnDisconnect() }
        case "joined":
            if inThisCall,
               actor.lowercased() != nick.lowercased(),
               !callParticipants.contains(where: { $0.lowercased() == actor.lowercased() }) {
                callParticipants.append(actor)
            }
        case "left":
            if inThisCall {
                callParticipants.removeAll { $0.lowercased() == actor.lowercased() }
                participantsWithVideo = participantsWithVideo.filter { $0.lowercased() != actor.lowercased() }
            }
        default:
            break
        }
    }
}

// MARK: - AV Event Handler

final class AvCallbackHandler: @unchecked Sendable, AvEventHandler {
    private weak var appState: AppState?

    init(appState: AppState) {
        self.appState = appState
    }

    func onAvEvent(event: AvEvent) {
        if Thread.isMainThread {
            handle(event: event)
        } else {
            DispatchQueue.main.async { [weak self] in self?.handle(event: event) }
        }
    }

    private func handle(event: AvEvent) {
        guard let state = appState else { return }
        switch event {
        case .connected:
            state.isInCall = true
        case .disconnected:
            state.tearDownCallLocallyOnDisconnect()
        case .participantJoined(let nick):
            if !state.callParticipants.contains(where: { $0.lowercased() == nick.lowercased() }) {
                state.callParticipants.append(nick)
            }
        case .participantLeft(let nick):
            state.callParticipants.removeAll { $0.lowercased() == nick.lowercased() }
            state.participantsWithVideo = state.participantsWithVideo.filter { $0.lowercased() != nick.lowercased() }
        case .audioTrackStarted, .audioTrackStopped, .videoTrackStarted:
            break
        case .videoTrackStopped(let nick):
            state.participantsWithVideo = state.participantsWithVideo.filter { $0.lowercased() != nick.lowercased() }
        case .videoFrame(let nick, let bgra, let width, let height):
            guard state.callParticipants.contains(where: { $0.lowercased() == nick.lowercased() }) else { return }
            if let layer = state.videoLayer(for: nick) {
                VideoSampleBuffer.enqueue(bgra: bgra, width: Int(width), height: Int(height), on: layer)
            }
            _ = state.participantsWithVideo.insert(nick)
        case .error(let message):
            print("[av] Error: \(message)")
        }
    }
}
