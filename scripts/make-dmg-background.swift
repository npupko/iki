// Draw the DMG window background: the wordmark, an indigo arrow from the app
// to Applications with a short caption under it, on cream paper.
//
//   swift scripts/make-dmg-background.swift <wordmark.png> <out-dir>
//
// Writes background.png (640×400) and background@2x.png. The icon centres in
// scripts/dmg-settings.py must match `appX`, `appsX` and `iconY` below.

import AppKit

let width: CGFloat = 640
let height: CGFloat = 400
let appX: CGFloat = 170
let appsX: CGFloat = 470
let iconY: CGFloat = 205  // from the top, as Finder counts

let paper = NSColor(srgbRed: 0xF6 / 255, green: 0xF1 / 255, blue: 0xE6 / 255, alpha: 1)
let indigo = NSColor(srgbRed: 0x2E / 255, green: 0x4A / 255, blue: 0x6B / 255, alpha: 1)
let sumi = NSColor(srgbRed: 0x1B / 255, green: 0x1B / 255, blue: 0x1A / 255, alpha: 1)

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

let args = CommandLine.arguments
guard args.count == 3 else { fail("usage: make-dmg-background.swift <wordmark.png> <out-dir>") }
guard let wordmark = NSImage(contentsOfFile: args[1]) else { fail("cannot read \(args[1])") }
let outDir = URL(fileURLWithPath: args[2])

/// Flip from Finder's top-left coordinates to AppKit's bottom-left.
func y(_ top: CGFloat) -> CGFloat { height - top }

func draw() {
    paper.setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()

    // Wordmark, centred near the top.
    let markHeight: CGFloat = 46
    let markWidth = wordmark.size.width * markHeight / wordmark.size.height
    wordmark.draw(in: NSRect(x: (width - markWidth) / 2, y: y(34 + markHeight),
                             width: markWidth, height: markHeight))

    // A gentle brushed arc between the two icons, then an open arrowhead.
    let start = CGPoint(x: appX + 88, y: y(iconY))
    let end = CGPoint(x: appsX - 92, y: y(iconY))
    let arc = NSBezierPath()
    arc.move(to: start)
    arc.curve(to: end,
              controlPoint1: CGPoint(x: start.x + 40, y: start.y + 22),
              controlPoint2: CGPoint(x: end.x - 40, y: end.y + 22))
    arc.lineWidth = 5
    arc.lineCapStyle = .round
    indigo.setStroke()
    arc.stroke()

    // The head follows the curve's end tangent, which points along end - control2.
    let angle = atan2(-22.0, 40.0)
    let head = NSBezierPath()
    let length: CGFloat = 16
    for side in [-1.0, 1.0] {
        let a = angle + .pi + side * 0.55
        head.move(to: end)
        head.line(to: CGPoint(x: end.x + length * cos(a), y: end.y + length * sin(a)))
    }
    head.lineWidth = 5
    head.lineCapStyle = .round
    head.lineJoinStyle = .round
    head.stroke()

    // Caption under the arrow, clear of the Finder path and status bars.
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    let caption = NSAttributedString(string: "Drag to install", attributes: [
        .font: NSFont.systemFont(ofSize: 13, weight: .medium),
        .foregroundColor: sumi.withAlphaComponent(0.55),
        .paragraphStyle: style,
    ])
    caption.draw(in: NSRect(x: appX + 70, y: y(iconY + 44), width: appsX - appX - 140, height: 20))
}

func render(scale: CGFloat, to name: String) {
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(width * scale), pixelsHigh: Int(height * scale),
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
    else { fail("cannot allocate bitmap") }
    rep.size = NSSize(width: width, height: height)  // 72 dpi at 1x, 144 at 2x
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current?.imageInterpolation = .high
    draw()
    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else { fail("cannot encode \(name)") }
    do { try png.write(to: outDir.appendingPathComponent(name)) } catch { fail("\(error)") }
}

try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)
render(scale: 1, to: "background.png")
render(scale: 2, to: "background@2x.png")
print("Wrote background.png and background@2x.png to \(outDir.path)")
