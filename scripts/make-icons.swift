// Build the app icon and the menu bar glyph from one generated mark.
//
//   swift scripts/make-icons.swift <mark.png> <tile-hex> <glyph-hex> <out-dir>
//
// The mark is a flat tile with a light glyph brushed on it. Each pixel is
// split into "how much glyph" and "how much paper grain", so the same art
// can be recoloured into any palette without regenerating it.
//
// Writes AppIcon.icns, StatusIcon.png (18 px), StatusIcon@2x.png (36 px)
// and iki-mark.png (1024 px square, no rounding) into <out-dir>.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

struct RGB { var r, g, b: Double }

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

func hex(_ s: String) -> RGB {
    guard let v = UInt32(s.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16)
    else { fail("bad colour \(s)") }
    return RGB(r: Double(v >> 16 & 0xff) / 255, g: Double(v >> 8 & 0xff) / 255, b: Double(v & 0xff) / 255)
}

let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

func context(_ w: Int, _ h: Int) -> CGContext {
    CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: srgb,
              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
}

func load(_ path: String) -> CGImage {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { fail("cannot read \(path)") }
    return img
}

func save(_ img: CGImage, _ path: String) {
    let url = URL(fileURLWithPath: path)
    guard let dst = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
    else { fail("cannot write \(path)") }
    CGImageDestinationAddImage(dst, img, nil)
    if !CGImageDestinationFinalize(dst) { fail("cannot write \(path)") }
}

/// The mark as a straight RGBA buffer.
struct Pixels {
    let w: Int, h: Int
    var data: [UInt8]

    init(_ img: CGImage) {
        w = img.width; h = img.height
        let ctx = context(w, h)
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        data = Array(UnsafeBufferPointer(start: ctx.data!.assumingMemoryBound(to: UInt8.self), count: w * h * 4))
    }

    func rgb(_ i: Int) -> RGB {
        RGB(r: Double(data[i * 4]) / 255, g: Double(data[i * 4 + 1]) / 255, b: Double(data[i * 4 + 2]) / 255)
    }
}

func luma(_ c: RGB) -> Double { 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b }

func smoothstep(_ a: Double, _ b: Double, _ x: Double) -> Double {
    let t = min(max((x - a) / (b - a), 0), 1)
    return t * t * (3 - 2 * t)
}

/// Glyph coverage and paper grain for each pixel of the tile.
struct Mark {
    let w: Int, h: Int
    var alpha: [Double]
    var grain: [Double]
}

func analyse(_ px: Pixels) -> Mark {
    let n = px.w * px.h
    // The tile is the most common colour; the glyph is the brightest tenth.
    var bins: [Int: Int] = [:]
    for i in stride(from: 0, to: n, by: 7) {
        let c = px.rgb(i)
        bins[Int(c.r * 31) << 10 | Int(c.g * 31) << 5 | Int(c.b * 31), default: 0] += 1
    }
    let mode = bins.max { $0.value < $1.value }!.key
    var tile = RGB(r: 0, g: 0, b: 0), tileCount = 0.0
    var lumas: [Double] = []
    for i in stride(from: 0, to: n, by: 7) {
        let c = px.rgb(i)
        if Int(c.r * 31) << 10 | Int(c.g * 31) << 5 | Int(c.b * 31) == mode {
            tile.r += c.r; tile.g += c.g; tile.b += c.b; tileCount += 1
        }
        lumas.append(luma(c))
    }
    tile = RGB(r: tile.r / tileCount, g: tile.g / tileCount, b: tile.b / tileCount)
    let cut = lumas.sorted()[lumas.count * 9 / 10]
    var ink = RGB(r: 0, g: 0, b: 0), inkCount = 0.0
    for i in stride(from: 0, to: n, by: 7) where luma(px.rgb(i)) >= cut {
        let c = px.rgb(i); ink.r += c.r; ink.g += c.g; ink.b += c.b; inkCount += 1
    }
    ink = RGB(r: ink.r / inkCount, g: ink.g / inkCount, b: ink.b / inkCount)

    let d = RGB(r: ink.r - tile.r, g: ink.g - tile.g, b: ink.b - tile.b)
    let dd = d.r * d.r + d.g * d.g + d.b * d.b
    let tileLuma = luma(tile)
    var alpha = [Double](repeating: 0, count: n)
    var grain = [Double](repeating: 1, count: n)
    for i in 0..<n {
        let c = px.rgb(i)
        let t = ((c.r - tile.r) * d.r + (c.g - tile.g) * d.g + (c.b - tile.b) * d.b) / dd
        alpha[i] = smoothstep(0.25, 0.75, t)
        grain[i] = min(max(luma(c) / tileLuma, 0.95), 1.05)
    }

    // Light paper outside a rounded tile touches the border; glyph strokes do not.
    var queue: [Int] = []
    var seen = [Bool](repeating: false, count: n)
    for x in 0..<px.w { queue.append(x); queue.append((px.h - 1) * px.w + x) }
    for y in 0..<px.h { queue.append(y * px.w); queue.append(y * px.w + px.w - 1) }
    while let i = queue.popLast() {
        if seen[i] || alpha[i] < 0.2 { continue }
        seen[i] = true
        alpha[i] = 0; grain[i] = 1
        let x = i % px.w, y = i / px.w
        if x > 0 { queue.append(i - 1) }
        if x < px.w - 1 { queue.append(i + 1) }
        if y > 0 { queue.append(i - px.w) }
        if y < px.h - 1 { queue.append(i + px.w) }
    }
    return Mark(w: px.w, h: px.h, alpha: alpha, grain: grain)
}

/// Bounding box of pixels close to `tile`, so a margin of paper is cropped away.
func tileBounds(_ px: Pixels, _ mark: Mark) -> CGRect {
    var minX = px.w, minY = px.h, maxX = 0, maxY = 0
    let mid = px.rgb(px.w * (px.h / 2) + px.w / 8)
    for y in stride(from: 0, to: px.h, by: 2) {
        for x in stride(from: 0, to: px.w, by: 2) {
            let c = px.rgb(y * px.w + x)
            if abs(c.r - mid.r) + abs(c.g - mid.g) + abs(c.b - mid.b) < 0.12 {
                minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
            }
        }
    }
    let side = min(maxX - minX, maxY - minY)
    let inset = Double(side) * 0.01
    return CGRect(x: Double(minX + maxX - side) / 2 + inset, y: Double(minY + maxY - side) / 2 + inset,
                  width: Double(side) - 2 * inset, height: Double(side) - 2 * inset)
}

func glyphBounds(_ mark: Mark) -> CGRect {
    var minX = mark.w, minY = mark.h, maxX = 0, maxY = 0
    for y in 0..<mark.h {
        for x in 0..<mark.w where mark.alpha[y * mark.w + x] > 0.5 {
            minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
        }
    }
    return CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1)
}

/// Recolour the mark into a palette. `glyphOnly` keeps just the glyph, in
/// `ink`, on transparency.
func render(_ mark: Mark, tile: RGB, ink: RGB, glyphOnly: Bool = false) -> CGImage {
    var out = [UInt8](repeating: 0, count: mark.w * mark.h * 4)
    for i in 0..<(mark.w * mark.h) {
        let a = mark.alpha[i], g = mark.grain[i]
        let o = glyphOnly ? a : 1
        let r = glyphOnly ? ink.r : (1 - a) * tile.r * g + a * ink.r
        let gg = glyphOnly ? ink.g : (1 - a) * tile.g * g + a * ink.g
        let b = glyphOnly ? ink.b : (1 - a) * tile.b * g + a * ink.b
        out[i * 4] = UInt8(min(max(r * o, 0), 1) * 255)
        out[i * 4 + 1] = UInt8(min(max(gg * o, 0), 1) * 255)
        out[i * 4 + 2] = UInt8(min(max(b * o, 0), 1) * 255)
        out[i * 4 + 3] = UInt8(o * 255)
    }
    let provider = CGDataProvider(data: Data(out) as CFData)!
    return CGImage(width: mark.w, height: mark.h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: mark.w * 4,
                   space: srgb, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                   provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)!
}

/// The glyph's longer side as a share of the tile. Generated art frames the
/// glyph loosely, so it is rescaled around the glyph's centre and the rest
/// of the tile is plain tile colour.
let glyphShare = 0.64

func drawArt(_ ctx: CGContext, _ art: CGImage, crop: CGRect, glyph: CGRect, tile: RGB, in rect: CGRect) {
    ctx.setFillColor(CGColor(srgbRed: tile.r, green: tile.g, blue: tile.b, alpha: 1))
    ctx.fill(rect)
    let k = rect.width * glyphShare / max(glyph.width, glyph.height)
    let u = glyph.midX - crop.minX, v = glyph.midY - crop.minY
    let w = crop.width * k, h = crop.height * k
    ctx.draw(art.cropping(to: crop)!, in: CGRect(x: rect.midX - u * k, y: rect.midY + v * k - h, width: w, height: h))
}

/// macOS icon grid: an 824 pt rounded tile centred on a 1024 pt canvas.
func appIcon(_ art: CGImage, crop: CGRect, glyph: CGRect, tile tileColor: RGB, size: Int) -> CGImage {
    let s = Double(size) / 1024
    let ctx = context(size, size)
    ctx.interpolationQuality = .high
    let tile = CGRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
    let path = CGPath(roundedRect: tile, cornerWidth: 185.4 * s, cornerHeight: 185.4 * s, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10 * s), blur: 20 * s,
                  color: CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.3))
    ctx.addPath(path)
    ctx.setFillColor(CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 1))
    ctx.fillPath()
    ctx.restoreGState()
    ctx.addPath(path)
    ctx.clip()
    drawArt(ctx, art, crop: crop, glyph: glyph, tile: tileColor, in: tile)
    return ctx.makeImage()!
}

/// Template glyph for the menu bar: black, alpha only, fitted to the square.
func statusIcon(_ glyph: CGImage, crop: CGRect, size: Int) -> CGImage {
    let ctx = context(size, size)
    ctx.interpolationQuality = .high
    let pad = Double(size) / 18
    let box = Double(size) - 2 * pad
    let k = box / max(crop.width, crop.height)
    let w = crop.width * k, h = crop.height * k
    ctx.draw(glyph.cropping(to: crop)!, in: CGRect(x: (Double(size) - w) / 2, y: (Double(size) - h) / 2, width: w, height: h))
    return ctx.makeImage()!
}

let args = CommandLine.arguments
guard args.count == 5 else { fail("usage: make-icons.swift <mark.png> <tile-hex> <glyph-hex> <out-dir>") }
let px = Pixels(load(args[1]))
let mark = analyse(px)
let tile = hex(args[2]), ink = hex(args[3])
let out = args[4]
try FileManager.default.createDirectory(atPath: out, withIntermediateDirectories: true)

let art = render(mark, tile: tile, ink: ink)
let crop = tileBounds(px, mark)
let glyph = render(mark, tile: tile, ink: RGB(r: 0, g: 0, b: 0), glyphOnly: true)
let glyphBox = glyphBounds(mark)

let flat = context(1024, 1024)
flat.interpolationQuality = .high
drawArt(flat, art, crop: crop, glyph: glyphBox, tile: tile, in: CGRect(x: 0, y: 0, width: 1024, height: 1024))
save(flat.makeImage()!, "\(out)/iki-mark.png")
save(statusIcon(glyph, crop: glyphBox, size: 18), "\(out)/StatusIcon.png")
save(statusIcon(glyph, crop: glyphBox, size: 36), "\(out)/StatusIcon@2x.png")

let iconset = "\(out)/AppIcon.iconset"
try? FileManager.default.removeItem(atPath: iconset)
try FileManager.default.createDirectory(atPath: iconset, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    save(appIcon(art, crop: crop, glyph: glyphBox, tile: tile, size: base), "\(iconset)/icon_\(base)x\(base).png")
    save(appIcon(art, crop: crop, glyph: glyphBox, tile: tile, size: base * 2), "\(iconset)/icon_\(base)x\(base)@2x.png")
}
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset, "-o", "\(out)/AppIcon.icns"]
try iconutil.run()
iconutil.waitUntilExit()
if iconutil.terminationStatus != 0 { fail("iconutil failed") }
try FileManager.default.removeItem(atPath: iconset)
print("wrote \(out)")
