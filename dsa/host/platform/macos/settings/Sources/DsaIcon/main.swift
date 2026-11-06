// Draws DSA's placeholder app icon into an .iconset folder, at every size the .icns holds:
// 16, 32, 128, 256 and 512 px at 1x and 2x, so 16 to 1024 px. `iconutil -c icns` turns the
// folder into the .icns that DSA.app carries.
//
// The picture follows Apple's icon grid: a rounded square (squircle) body inside the canvas
// with a soft shadow, a gradient, and the DSA placeholder symbol, two app windows one over the
// other. The symbol's design is not chosen yet; when it is, this drawing is what changes.
//
// Usage: dsa-icon <output .iconset folder>

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

func draw(size: Int) -> CGImage? {
    let s = CGFloat(size)
    guard let context = CGContext(
        data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        return nil
    }
    // Apple's grid: an 824 px body on a 1024 px canvas, with corners of about 185 px.
    let body = CGRect(x: s * 100 / 1024, y: s * 100 / 1024, width: s * 824 / 1024, height: s * 824 / 1024)
    let radius = body.width * 0.2237
    let shape = CGPath(roundedRect: body, cornerWidth: radius, cornerHeight: radius, transform: nil)

    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -s * 10 / 1024), blur: s * 28 / 1024, color: color(0x000000, 0.3))
    context.addPath(shape)
    context.setFillColor(color(0x1F7AE0))
    context.fillPath()
    context.restoreGState()

    context.saveGState()
    context.addPath(shape)
    context.clip()
    let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [color(0x2BC4B4), color(0x2257E8)] as CFArray,
                              locations: [0, 1])!
    context.drawLinearGradient(gradient, start: CGPoint(x: body.minX, y: body.maxY), end: CGPoint(x: body.maxX, y: body.minY), options: [])
    context.restoreGState()

    // The symbol: a window outlined behind, a window filled in front.
    let unit = body.width / 16
    let line = max(1, unit * 0.9)
    let back = CGRect(x: body.minX + unit * 3, y: body.minY + unit * 6.5, width: unit * 8, height: unit * 6.5)
    let front = CGRect(x: body.minX + unit * 5, y: body.minY + unit * 3, width: unit * 8, height: unit * 6.5)
    context.setStrokeColor(color(0xFFFFFF, 0.9))
    context.setLineWidth(line)
    context.addPath(CGPath(roundedRect: back.insetBy(dx: line / 2, dy: line / 2), cornerWidth: unit * 1.2, cornerHeight: unit * 1.2, transform: nil))
    context.strokePath()
    context.setFillColor(color(0xFFFFFF))
    context.addPath(CGPath(roundedRect: front, cornerWidth: unit * 1.2, cornerHeight: unit * 1.2, transform: nil))
    context.fillPath()
    // The front window's title band.
    context.setFillColor(color(0x2257E8, 0.35))
    context.fill(CGRect(x: front.minX + unit * 0.9, y: front.maxY - unit * 1.6, width: front.width - unit * 1.8, height: unit * 0.6))
    return context.makeImage()
}

func write(_ image: CGImage, to url: URL) -> Bool {
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        return false
    }
    CGImageDestinationAddImage(destination, image, nil)
    return CGImageDestinationFinalize(destination)
}

let args = CommandLine.arguments
guard args.count == 2 else {
    FileHandle.standardError.write(Data("usage: dsa-icon <output .iconset folder>\n".utf8))
    exit(2)
}
let folder = URL(fileURLWithPath: args[1], isDirectory: true)
try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
        guard let image = draw(size: points * scale), write(image, to: folder.appendingPathComponent(name)) else {
            FileHandle.standardError.write(Data("dsa-icon: could not draw \(name)\n".utf8))
            exit(1)
        }
    }
}
