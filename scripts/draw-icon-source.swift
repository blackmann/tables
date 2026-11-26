// Draws assets/icon-source.png for the Tables icon: a ridgeline plot whose
// ridges fade from red (top) through yellow to green (bottom), each peak
// drifting right, over faint columns of numbers. Square and full-bleed; run
// scripts/make-icon.sh afterwards to build AppIcon.icns from it.
// Usage: swift scripts/draw-icon-source.swift <out.png> [size]
import AppKit
import CoreGraphics
import Foundation

let args = CommandLine.arguments
let out = args.count > 1 ? args[1] : "assets/icon-source.png"
let size = args.count > 2 ? CGFloat(Double(args[2])!) : 1024
let s = size / 1024

let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8,
                    bytesPerRow: 0, space: cs,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
// Work in a 1024pt, top-left-origin space.
ctx.translateBy(x: 0, y: size)
ctx.scaleBy(x: s, y: -s)

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: cs, components: [CGFloat((hex >> 16) & 0xff) / 255,
                                         CGFloat((hex >> 8) & 0xff) / 255,
                                         CGFloat(hex & 0xff) / 255, a])!
}

// The artwork was laid out on Apple's 824pt icon body at (100,100); stretch that
// body to fill the canvas. make-icon.swift adds the rounded corners and margin.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
ctx.scaleBy(x: 1024 / tile.width, y: 1024 / tile.height)
ctx.translateBy(x: -tile.minX, y: -tile.minY)
let bg = CGGradient(colorsSpace: cs, colors: [rgb(0xFFFDF8), rgb(0xF1ECE1)] as CFArray,
                    locations: [0, 1])!
ctx.drawLinearGradient(bg, start: CGPoint(x: 512, y: tile.minY), end: CGPoint(x: 512, y: tile.maxY),
                       options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])

// Faint columns of data down the right side, drawn behind the ridges.
do {
    var seed: UInt64 = 0x7AB1E5
    func rand() -> UInt64 { seed = seed &* 6364136223846793005 &+ 1442695040888963407; return seed >> 33 }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)
    let font = NSFont.monospacedSystemFont(ofSize: 30, weight: .medium)
    let rowH: CGFloat = 40
    let columns: [(right: CGFloat, fmt: (UInt64) -> String)] = [
        (872, { String(format: "%d.%02d", $0 % 10, $0 % 100) }),
        (770, { String(format: "%04d", $0 % 10000) }),
    ]
    for (ci, col) in columns.enumerated() {
        var y: CGFloat = 130 + CGFloat(ci) * 14
        while y < 900 {
            // Fade in at the top and out at the bottom of the tile.
            let edge = min((y - 120) / 120, (900 - y) / 120, 1)
            let alpha = 0.26 * max(edge, 0) * (ci == 0 ? 1 : 0.75)
            let text = NSAttributedString(string: col.fmt(rand()), attributes: [
                .font: font, .foregroundColor: NSColor(cgColor: rgb(0x2A2420, alpha))!,
            ])
            let w = text.size().width
            text.draw(at: CGPoint(x: col.right - w, y: y))
            y += rowH
        }
    }
    NSGraphicsContext.restoreGraphicsState()
}

// Ridges, top to bottom. Each: colour, then gaussian bumps (centre 0…1, height, width).
struct Bump { let c: CGFloat; let h: CGFloat; let w: CGFloat }
let ridges: [(UInt32, [Bump])] = [
    (0xE5392F, [Bump(c: 0.16, h: 1.00, w: 0.060), Bump(c: 0.34, h: 0.22, w: 0.10)]),
    (0xEE5A2C, [Bump(c: 0.24, h: 0.62, w: 0.080), Bump(c: 0.44, h: 0.42, w: 0.09)]),
    (0xF5892A, [Bump(c: 0.40, h: 0.70, w: 0.090), Bump(c: 0.22, h: 0.20, w: 0.07)]),
    (0xF8C12C, [Bump(c: 0.52, h: 1.05, w: 0.045), Bump(c: 0.40, h: 0.25, w: 0.08)]),
    (0xC9CF35, [Bump(c: 0.58, h: 0.62, w: 0.075), Bump(c: 0.70, h: 0.30, w: 0.07)]),
    (0x7DBE43, [Bump(c: 0.70, h: 0.72, w: 0.070), Bump(c: 0.84, h: 0.40, w: 0.06)]),
    (0x2FA34E, [Bump(c: 0.84, h: 1.00, w: 0.050), Bump(c: 0.68, h: 0.25, w: 0.07)]),
]

let left: CGFloat = 150, right: CGFloat = 874
let top: CGFloat = 360, bottom: CGFloat = 836
let peak: CGFloat = 200
let step = (bottom - top) / CGFloat(ridges.count - 1)
let ink = rgb(0x2A2420)

for (i, (hex, bumps)) in ridges.enumerated() {
    let base = top + CGFloat(i) * step
    let line = CGMutablePath()
    let n = 160
    for k in 0...n {
        let t = CGFloat(k) / CGFloat(n)
        var y: CGFloat = 0
        for b in bumps { y += b.h * exp(-pow(t - b.c, 2) / (2 * b.w * b.w)) }
        let p = CGPoint(x: left + t * (right - left), y: base - y * peak)
        if k == 0 { line.move(to: CGPoint(x: 0, y: p.y)); line.addLine(to: p) } else { line.addLine(to: p) }
    }
    line.addLine(to: CGPoint(x: 1024, y: base))

    // Fill: solid colour at the crest, fading toward the baseline (as in the reference).
    let area = line.mutableCopy()!
    area.addLine(to: CGPoint(x: 1024, y: base))
    area.addLine(to: CGPoint(x: 0, y: base))
    area.closeSubpath()
    ctx.saveGState()
    ctx.addPath(area)
    ctx.clip()
    let fill = CGGradient(colorsSpace: cs, colors: [rgb(hex), rgb(hex, 0.95), rgb(hex, 0.45)] as CFArray,
                          locations: [0, 0.6, 1])!
    ctx.drawLinearGradient(fill, start: CGPoint(x: 0, y: base - peak * 1.05),
                           end: CGPoint(x: 0, y: base), options: [])
    ctx.restoreGState()

    ctx.addPath(line)
    ctx.setStrokeColor(ink)
    ctx.setLineWidth(8)
    ctx.setLineJoin(.round)
    ctx.setLineCap(.round)
    ctx.strokePath()
}
let img = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: img)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
