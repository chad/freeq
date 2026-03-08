import UserNotifications
import UIKit

/// Handles local notification permissions and delivery.
/// Permission is deferred until the first mention/DM to avoid reflexive "Block".
class NotificationManager {
    static let shared = NotificationManager()

    private var authorized = false
    private var permissionRequested = false

    /// Request notification permission (idempotent — only asks once).
    func requestPermissionIfNeeded() {
        guard !permissionRequested else { return }
        permissionRequested = true
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
            DispatchQueue.main.async {
                self.authorized = granted
            }
        }
    }

    func sendMessageNotification(from: String, text: String, channel: String, isMention: Bool = false) {
        // Request permission on first notification attempt (deferred from app launch)
        if !permissionRequested {
            requestPermissionIfNeeded()
        }
        guard authorized else { return }
        // Don't notify if app is in foreground
        guard UIApplication.shared.applicationState != .active else { return }

        let content = UNMutableNotificationContent()
        if channel.hasPrefix("#") {
            content.title = channel
            content.subtitle = from
        } else {
            content.title = from
        }
        content.body = text
        // Mentions get a more prominent sound
        content.sound = isMention ? .defaultCritical : .default
        // Group by channel/DM
        content.threadIdentifier = channel
        content.categoryIdentifier = channel.hasPrefix("#") ? "channel_message" : "dm_message"
        content.userInfo = ["channel": channel, "from": from]
        // Summary for notification groups
        content.summaryArgument = channel.hasPrefix("#") ? channel : from

        let request = UNNotificationRequest(
            identifier: UUID().uuidString,
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(request)
    }

    func clearBadge() {
        UNUserNotificationCenter.current().setBadgeCount(0)
    }
}
