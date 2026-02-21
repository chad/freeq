import SwiftUI

/// Centralized color palette matching the web app's dark theme.
enum Theme {
    // Backgrounds
    static let bgPrimary = Color(hex: "1a1a2e")
    static let bgSecondary = Color(hex: "16162a")
    static let bgTertiary = Color(hex: "222240")
    static let bgHover = Color(hex: "2a2a4a")

    // Text
    static let textPrimary = Color(hex: "e8e8f0")
    static let textSecondary = Color(hex: "8888aa")
    static let textMuted = Color(hex: "666688")

    // Accent
    static let accent = Color(hex: "6c63ff")
    static let accentLight = Color(hex: "8b83ff")

    // Status
    static let success = Color(hex: "43b581")
    static let warning = Color(hex: "faa61a")
    static let danger = Color(hex: "f04747")

    // Borders
    static let border = Color(hex: "2a2a4a")

    // Nick colors (deterministic by name)
    static let nickColors: [Color] = [
        Color(hex: "6c63ff"),
        Color(hex: "43b581"),
        Color(hex: "faa61a"),
        Color(hex: "f04747"),
        Color(hex: "e91e8c"),
        Color(hex: "1abc9c"),
        Color(hex: "e67e22"),
        Color(hex: "3498db"),
        Color(hex: "9b59b6"),
        Color(hex: "2ecc71"),
    ]

    static func nickColor(for nick: String) -> Color {
        let hash = nick.unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
        return nickColors[abs(hash) % nickColors.count]
    }
}

extension Color {
    init(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 6:
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8:
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            (a, r, g, b) = (255, 0, 0, 0)
        }
        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue: Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}
