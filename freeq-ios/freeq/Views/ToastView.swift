import SwiftUI

/// Lightweight toast notification overlay.
struct ToastView: View {
    let message: String
    let icon: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(.white)
            Text(message)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(.white)
                .lineLimit(2)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .background(.ultraThinMaterial)
        .background(Theme.bgTertiary.opacity(0.9))
        .cornerRadius(24)
        .shadow(color: .black.opacity(0.3), radius: 12, y: 6)
    }
}

/// Observable toast state — add to environment.
@MainActor
class ToastManager: ObservableObject {
    static let shared = ToastManager()

    @Published var currentToast: (message: String, icon: String)? = nil
    private var hideTask: Task<Void, Never>?

    func show(_ message: String, icon: String = "checkmark.circle.fill") {
        hideTask?.cancel()
        currentToast = (message, icon)
        UIImpactFeedbackGenerator(style: .light).impactOccurred()

        hideTask = Task {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            if !Task.isCancelled {
                withAnimation(.easeOut(duration: 0.3)) {
                    currentToast = nil
                }
            }
        }
    }
}

/// Modifier to overlay toast on any view.
struct ToastOverlay: ViewModifier {
    @ObservedObject var manager = ToastManager.shared

    func body(content: Content) -> some View {
        ZStack {
            content
            VStack {
                if let toast = manager.currentToast {
                    ToastView(message: toast.message, icon: toast.icon)
                        .transition(.move(edge: .top).combined(with: .opacity))
                        .padding(.top, 8)
                }
                Spacer()
            }
            .animation(.spring(response: 0.3, dampingFraction: 0.8), value: manager.currentToast != nil)
        }
    }
}

extension View {
    func withToast() -> some View {
        modifier(ToastOverlay())
    }
}
