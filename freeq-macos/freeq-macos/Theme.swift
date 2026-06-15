import SwiftUI

/// Design tokens for freeq macOS — follows system appearance automatically.
enum Theme {
    // Brand
    static let accent = Color(red: 0.00, green: 0.58, blue: 0.50)
    static let accentSoft = Color(red: 0.88, green: 0.97, blue: 0.95)
    static let blue = Color(red: 0.22, green: 0.47, blue: 0.88)
    static let purple = Color(red: 0.48, green: 0.38, blue: 0.86)

    // Backgrounds — warm light by default.
    static let appBackground = Color(red: 0.965, green: 0.965, blue: 0.950)
    static let sidebarBackground = Color(red: 0.945, green: 0.948, blue: 0.930)
    static let chatBackground = Color(red: 0.992, green: 0.990, blue: 0.980)
    static let detailBackground = Color(red: 0.972, green: 0.972, blue: 0.955)
    static let surface = Color.white
    static let surfaceSoft = Color(red: 0.982, green: 0.980, blue: 0.966)
    static let surfaceElevated = Color.white

    // Text
    static let textPrimary = Color(red: 0.105, green: 0.110, blue: 0.125)
    static let textSecondary = Color(red: 0.390, green: 0.400, blue: 0.430)
    static let textTertiary = Color(red: 0.575, green: 0.580, blue: 0.610)

    // Semantic
    static let success = Color(red: 0.10, green: 0.68, blue: 0.34)
    static let warning = Color(red: 0.88, green: 0.50, blue: 0.12)
    static let danger = Color(red: 0.84, green: 0.18, blue: 0.20)
    static let verified = Color(red: 0.18, green: 0.42, blue: 0.92)

    // Border
    static let border = Color(red: 0.845, green: 0.845, blue: 0.825)
    static let borderSoft = Color(red: 0.910, green: 0.908, blue: 0.890)
    static let hairline = Color.black.opacity(0.06)

    // Messages
    static let outgoingBubble = Color(red: 0.220, green: 0.455, blue: 0.835)
    static let incomingBubble = Color(red: 0.935, green: 0.932, blue: 0.910)
    static let systemPill = Color(red: 0.925, green: 0.940, blue: 0.955)

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
