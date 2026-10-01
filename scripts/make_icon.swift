// Draws the VoiceBridge app icon (1024×1024 PNG, transparent corners).
// Usage: swift scripts/make_icon.swift src-tauri/app-icon.png
import AppKit
import CoreGraphics

let size: CGFloat = 1024
let space = CGColorSpaceCreateDeviceRGB()
let ctx = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
                    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: space, components: [r / 255, g / 255, b / 255, a])!
}

// macOS icon grid: an 824 pt rounded square centred in the 1024 canvas.
let body = CGRect(x: 100, y: 100, width: 824, height: 824)
let shape = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)

// Soft drop shadow.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: color(0, 0, 0, 0.30))
ctx.addPath(shape)
ctx.setFillColor(color(16, 24, 40))
ctx.fillPath()
ctx.restoreGState()

// Background gradient: deep navy to teal-green.
ctx.saveGState()
ctx.addPath(shape)
ctx.clip()
let gradient = CGGradient(colorsSpace: space,
                          colors: [color(13, 20, 36), color(18, 52, 74), color(22, 118, 104)] as CFArray,
                          locations: [0, 0.55, 1])!
ctx.drawLinearGradient(gradient, start: CGPoint(x: body.minX, y: body.maxY), end: CGPoint(x: body.maxX, y: body.minY), options: [])
// Subtle top highlight.
let gloss = CGGradient(colorsSpace: space, colors: [color(255, 255, 255, 0.10), color(255, 255, 255, 0)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(gloss, start: CGPoint(x: 512, y: body.maxY), end: CGPoint(x: 512, y: 560), options: [])
ctx.restoreGState()

// Voice: a symmetric waveform of rounded bars.
let heights: [CGFloat] = [120, 230, 350, 440, 350, 230, 120]
let barWidth: CGFloat = 56
let gap: CGFloat = 34
let total = CGFloat(heights.count) * barWidth + CGFloat(heights.count - 1) * gap
let centerY: CGFloat = 560
var x = (size - total) / 2
for (i, h) in heights.enumerated() {
    let rect = CGRect(x: x, y: centerY - h / 2, width: barWidth, height: h)
    let bar = CGPath(roundedRect: rect, cornerWidth: barWidth / 2, cornerHeight: barWidth / 2, transform: nil)
    ctx.addPath(bar)
    // Centre bar is the accent: the moment speech becomes a prompt.
    ctx.setFillColor(i == heights.count / 2 ? color(94, 234, 180) : color(255, 255, 255, 0.95))
    ctx.fillPath()
    x += barWidth + gap
}

// Bridge: an arch under the waveform, spanning from one side to the other.
let left = (size - total) / 2 + barWidth / 2
let right = size - left
let deckY: CGFloat = 232
ctx.setLineCap(.round)
ctx.setLineWidth(30)
ctx.setStrokeColor(color(94, 234, 180))
ctx.move(to: CGPoint(x: left, y: deckY))
ctx.addQuadCurve(to: CGPoint(x: right, y: deckY), control: CGPoint(x: 512, y: deckY + 120))
ctx.strokePath()
for px in [left, right] {
    ctx.addEllipse(in: CGRect(x: px - 27, y: deckY - 27, width: 54, height: 54))
}
ctx.setFillColor(color(255, 255, 255, 0.95))
ctx.fillPath()

let image = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: image)
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "app-icon.png"
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
