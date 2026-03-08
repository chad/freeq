import SwiftUI

/// Design tokens for freeq macOS — follows system appearance automatically.
enum Theme {
    // Backgrounds
    static let bgPrimary = Color("bgPrimary", bundle: nil)
    static let bgSecondary = Color(nsColor: .controlBackgroundColor)
    static let bgTertiary = Color(nsColor: .underPageBackgroundColor)
    static let surface = Color(nsColor: .windowBackgroundColor)

    // Text
    static let textPrimary = Color(nsColor: .labelColor)
    static let textSecondary = Color(nsColor: .secondaryLabelColor)
    static let textTertiary = Color(nsColor: .tertiaryLabelColor)

    // Accent — uses system accent color
    static let accent = Color.accentColor

    // Semantic
    static let success = Color.green
    static let warning = Color.orange
    static let danger = Color.red

    // Border
    static let border = Color(nsColor: .separatorColor)

    // Nick colors (consistent with web + iOS)
    static let nickColors: [Color] = [
        Color(red: 1.0, green: 0.43, blue: 0.71),    // #ff6eb4
        Color(red: 0.0, green: 0.83, blue: 0.67),     // #00d4aa
        Color(red: 1.0, green: 0.71, blue: 0.28),     // #ffb547
        Color(red: 0.36, green: 0.62, blue: 1.0),     // #5c9eff
        Color(red: 0.69, green: 0.55, blue: 1.0),     // #b18cff
        Color(red: 1.0, green: 0.58, blue: 0.28),     // #ff9547
        Color(red: 0.0, green: 0.77, blue: 1.0),      // #00c4ff
        Color(red: 1.0, green: 0.36, blue: 0.36),     // #ff5c5c
        Color(red: 0.49, green: 0.87, blue: 0.49),    // #7edd7e
        Color(red: 1.0, green: 0.52, blue: 0.82),     // #ff85d0
    ]

    static func nickColor(for nick: String) -> Color {
        var h: Int = 0
        for char in nick.unicodeScalars {
            h = Int(char.value) &+ ((h &<< 5) &- h)
        }
        return nickColors[abs(h) % nickColors.count]
    }
}
