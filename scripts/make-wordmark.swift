// Turn the generated lockup into README wordmarks on transparency.
//
//   swift scripts/make-wordmark.swift <lockup.png> <accent-from-hex> <accent-to-hex> <out-dir>
//
// The lockup is sumi letters and an accent seal on flat paper. The accent is
// recoloured, the paper is unmixed into alpha so edges carry the letter colour
// instead of a paper halo, and the seal's inside is kept as drawn.
//
// Writes iki-wordmark.png (sumi letters) and iki-wordmark-dark.png (cream letters).

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

func sub(_ a: RGB, _ b: RGB) -> RGB { RGB(r: a.r - b.r, g: a.g - b.g, b: a.b - b.b) }
func dot(_ a: RGB, _ b: RGB) -> Double { a.r * b.r + a.g * b.g + a.b * b.b }
func mix(_ a: RGB, _ b: RGB, _ t: Double) -> RGB {
    RGB(r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t)
}

/// Where `p` sits on the segment from `a` (0) to `b` (1), and how far off it.
func project(_ p: RGB, _ a: RGB, _ b: RGB) -> (t: Double, d: Double) {
    let ab = sub(b, a)
    let t = min(max(dot(sub(p, a), ab) / dot(ab, ab), 0), 1)
    let off = sub(p, mix(a, b, t))
    return (t, dot(off, off).squareRoot())
}

let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

func load(_ path: String) -> CGImage {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { fail("cannot read \(path)") }
    return img
}

func save(_ img: CGImage, _ path: String) {
    guard let dst = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL,
                                                    UTType.png.identifier as CFString, 1, nil)
    else { fail("cannot write \(path)") }
    CGImageDestinationAddImage(dst, img, nil)
    if !CGImageDestinationFinalize(dst) { fail("cannot write \(path)") }
}

let args = CommandLine.arguments
guard args.count == 5 else { fail("usage: make-wordmark.swift <lockup.png> <accent-from-hex> <accent-to-hex> <out-dir>") }
let img = load(args[1])
let w = img.width, h = img.height, n = w * h
let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: srgb,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
let raw = UnsafeBufferPointer(start: ctx.data!.assumingMemoryBound(to: UInt8.self), count: n * 4)
let px = (0..<n).map { RGB(r: Double(raw[$0 * 4]) / 255, g: Double(raw[$0 * 4 + 1]) / 255, b: Double(raw[$0 * 4 + 2]) / 255) }

let accentFrom = hex(args[2]), accentTo = hex(args[3])
let sumi = hex("1B1B1A"), cream = hex("F0E8D5")

// Paper is the most common colour, ink the darkest few percent.
var bins: [Int: (RGB, Int)] = [:]
for i in stride(from: 0, to: n, by: 5) {
    let c = px[i], key = Int(c.r * 31) << 10 | Int(c.g * 31) << 5 | Int(c.b * 31)
    let (sum, count) = bins[key] ?? (RGB(r: 0, g: 0, b: 0), 0)
    bins[key] = (RGB(r: sum.r + c.r, g: sum.g + c.g, b: sum.b + c.b), count + 1)
}
let (paperSum, paperCount) = bins.values.max { $0.1 < $1.1 }!
let paper = RGB(r: paperSum.r / Double(paperCount), g: paperSum.g / Double(paperCount), b: paperSum.b / Double(paperCount))
let ink = px.sorted { dot($0, $0) < dot($1, $1) }[n / 200]

// Coverage over paper, and which foreground each pixel belongs to.
var alpha = [Double](repeating: 0, count: n)
var isAccent = [Bool](repeating: false, count: n)
for i in 0..<n {
    let viaInk = project(px[i], paper, ink), viaAccent = project(px[i], paper, accentFrom)
    if viaAccent.d < viaInk.d {
        alpha[i] = viaAccent.t; isAccent[i] = true
    } else {
        alpha[i] = viaInk.t
    }
}

// Paper reachable from the border is background; the seal's inside is not.
var outside = [Bool](repeating: false, count: n)
var queue: [Int] = []
for x in 0..<w { queue.append(x); queue.append((h - 1) * w + x) }
for y in 0..<h { queue.append(y * w); queue.append(y * w + w - 1) }
while let i = queue.popLast() {
    if outside[i] || alpha[i] > 0.9 { continue }
    outside[i] = true
    let x = i % w, y = i / w
    if x > 0 { queue.append(i - 1) }
    if x < w - 1 { queue.append(i + 1) }
    if y > 0 { queue.append(i - w) }
    if y < h - 1 { queue.append(i + w) }
}

func render(letters: RGB) -> CGImage {
    var minX = w, minY = h, maxX = 0, maxY = 0
    var out = [UInt8](repeating: 0, count: n * 4)
    for i in 0..<n {
        var c: RGB, a: Double
        if !outside[i] {
            // Inside the seal: cream glyph over the accent, recoloured in place.
            let p = project(px[i], accentFrom, cream)
            c = p.d < 0.12 ? mix(accentTo, cream, p.t) : (isAccent[i] ? accentTo : letters)
            a = 1
        } else {
            a = alpha[i] < 0.08 ? 0 : alpha[i]
            c = isAccent[i] ? accentTo : letters
        }
        if a > 0.5 {
            let x = i % w, y = i / w
            minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
        }
        out[i * 4] = UInt8(c.r * a * 255)
        out[i * 4 + 1] = UInt8(c.g * a * 255)
        out[i * 4 + 2] = UInt8(c.b * a * 255)
        out[i * 4 + 3] = UInt8(a * 255)
    }
    let full = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4, space: srgb,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                       provider: CGDataProvider(data: Data(out) as CFData)!, decode: nil,
                       shouldInterpolate: true, intent: .defaultIntent)!
    let pad = (maxY - minY) / 12
    return full.cropping(to: CGRect(x: minX - pad, y: minY - pad, width: maxX - minX + 2 * pad, height: maxY - minY + 2 * pad))!
}

let dir = args[4]
try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
save(render(letters: sumi), "\(dir)/iki-wordmark.png")
save(render(letters: cream), "\(dir)/iki-wordmark-dark.png")
print("wrote \(dir)")
