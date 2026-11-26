// Renders assets/icon-source.* into a macOS-style 1024px app icon master:
// artwork clipped to a rounded rect on Apple's icon grid (824px body, transparent margin).
import AppKit

let args = CommandLine.arguments
guard args.count == 3, let src = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: make-icon.swift <source> <out.png>\n".data(using: .utf8)!)
    exit(1)
}
let size = 1024, body: CGFloat = 824, radius: CGFloat = 185
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                    space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let inset = (CGFloat(size) - body) / 2
let rect = CGRect(x: inset, y: inset, width: body, height: body)
ctx.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
ctx.clip()
ctx.interpolationQuality = .high
var r = CGRect(origin: .zero, size: src.size)
ctx.draw(src.cgImage(forProposedRect: &r, context: nil, hints: nil)!, in: rect)
let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
