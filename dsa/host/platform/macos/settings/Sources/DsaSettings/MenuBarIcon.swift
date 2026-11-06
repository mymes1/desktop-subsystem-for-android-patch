import AppKit

/// The menu bar item's icon: a template image (one colour on a clear background), so the
/// system colours it for a light or a dark menu bar. It is the DSA placeholder symbol, two
/// app windows one over the other, until the owner picks the symbol's design.
enum MenuBarIcon {
    static let template: NSImage = {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { _ in
            NSColor.black.setStroke()
            NSColor.black.setFill()
            let back = NSBezierPath(roundedRect: NSRect(x: 2, y: 6, width: 10, height: 9), xRadius: 2, yRadius: 2)
            back.lineWidth = 1.5
            back.stroke()
            let front = NSBezierPath(roundedRect: NSRect(x: 6, y: 2, width: 10, height: 9), xRadius: 2, yRadius: 2)
            front.fill()
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "DSA"
        return image
    }()
}
