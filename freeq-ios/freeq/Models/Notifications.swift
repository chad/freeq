import UserNotifications
import UIKit

/// Handles local notification permissions and delivery.
class NotificationManager {
    static let shared = NotificationManager()

    private var authorized = false

    func requestPermission() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
            DispatchQueue.main.async {
                self.authorized = granted
            }
        }
    }

    func sendMessageNotification(from: String, text: String, channel: String) {
        guard authorized else { return }
        // Don't notify if app is in foreground
        guard UIApplication.shared.applicationState != .active else { return }

        let content = UNMutableNotificationContent()
        content.title = channel.hasPrefix("#") ? "\(from) in \(channel)" : from
        content.body = text
        content.sound = .default
        content.threadIdentifier = channel

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
