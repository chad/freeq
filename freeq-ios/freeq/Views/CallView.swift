import SwiftUI

/// Voice/video call overlay — shown when the user is in an AV session.
/// Camera is off by default (audio only). Tap the camera button to enable video.
struct CallView: View {
    @EnvironmentObject var appState: AppState
    let channel: String

    var body: some View {
        VStack(spacing: 0) {
            // Participant grid (when video is on or participants exist)
            if appState.isInCall {
                participantGrid
            }

            // Controls bar
            if appState.isInCall {
                controlsBar
            }
        }
        .background(Color(.systemBackground).opacity(0.95))
    }

    private var participantGrid: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                // Local tile — shows live camera preview when on, avatar when off.
                VStack(spacing: 4) {
                    ZStack {
                        RoundedRectangle(cornerRadius: 8)
                            .fill(Color(.secondarySystemBackground))
                            .frame(width: 100, height: 75)

                        if appState.isCameraOn, let cap = appState.localPreviewCapture {
                            LocalPreviewView(capture: cap)
                                .frame(width: 100, height: 75)
                                .clipShape(RoundedRectangle(cornerRadius: 8))
                        } else {
                            Text(String(appState.currentNick?.prefix(2).uppercased() ?? "Me"))
                                .font(.title2)
                                .fontWeight(.bold)
                                .foregroundColor(.accentColor)
                        }
                    }

                    Text("You")
                        .font(.caption2)
                        .foregroundColor(.secondary)
                }

                // Remote participants — video tile when frames are arriving,
                // avatar otherwise. The tile always registers a display sink so
                // the next inbound frame can drive it.
                ForEach(appState.callParticipants, id: \.self) { nick in
                    VStack(spacing: 4) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 8)
                                .fill(Color(.secondarySystemBackground))
                                .frame(width: 100, height: 75)

                            RemoteVideoTile(appState: appState, nick: nick)
                                .frame(width: 100, height: 75)
                                .clipShape(RoundedRectangle(cornerRadius: 8))
                                .opacity(appState.participantsWithVideo.contains(nick) ? 1 : 0)

                            if !appState.participantsWithVideo.contains(nick) {
                                Text(String(nick.prefix(2).uppercased()))
                                    .font(.title2)
                                    .fontWeight(.bold)
                                    .foregroundColor(.accentColor)
                            }
                        }

                        Text(nick)
                            .font(.caption2)
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
    }

    private var controlsBar: some View {
        HStack(spacing: 16) {
            // Status
            HStack(spacing: 6) {
                Circle()
                    .fill(Color.green)
                    .frame(width: 8, height: 8)

                Text("Voice (\(appState.callParticipants.count + 1))")
                    .font(.subheadline)
                    .fontWeight(.medium)
                    .foregroundColor(.green)
            }

            Spacer()

            // Mute
            Button(action: { appState.toggleMute() }) {
                Image(systemName: appState.isMuted ? "mic.slash.fill" : "mic.fill")
                    .font(.system(size: 16))
                    .foregroundColor(.white)
                    .frame(width: 40, height: 40)
                    .background(appState.isMuted ? Color.red : Color(.systemGray4))
                    .clipShape(Circle())
            }

            // Camera
            Button(action: { appState.toggleCamera() }) {
                Image(systemName: appState.isCameraOn ? "video.fill" : "video.slash.fill")
                    .font(.system(size: 16))
                    .foregroundColor(.white)
                    .frame(width: 40, height: 40)
                    .background(appState.isCameraOn ? Color.accentColor : Color(.systemGray4))
                    .clipShape(Circle())
            }

            // Leave
            Button(action: { appState.leaveCall() }) {
                Image(systemName: "phone.down.fill")
                    .font(.system(size: 16))
                    .foregroundColor(.white)
                    .frame(width: 40, height: 40)
                    .background(Color.red)
                    .clipShape(Circle())
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color(.secondarySystemBackground))
    }
}
