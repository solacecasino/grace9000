// Renders assets/icon.png (1024x1024) and assets/icon-256.png: the
// grace9000 mark, a photo's frame cut on the slant splice9000's strips are
// joined at, the lower part in grace violet, on the same graphite body as
// its siblings' icons.
// Run: swift scripts/make_icon.swift
import AppKit

func render(_ size: Int) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size,
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext
    let k = CGFloat(size) / 1024
    ctx.scaleBy(x: k, y: k)
    func color(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
        NSColor(srgbRed: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255,
                blue: CGFloat(hex & 0xff) / 255, alpha: a).cgColor
    }

    // Body on Apple's icon grid: 824 square, 100 inset.
    let body = CGPath(roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824), cornerWidth: 185, cornerHeight: 185, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: color(0, 0.35))
    ctx.addPath(body)
    ctx.setFillColor(color(0x17181b))
    ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addPath(body)
    ctx.clip()
    let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: [color(0x22242a), color(0x121316)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: CGPoint(x: 0, y: 924), end: CGPoint(x: 0, y: 100), options: [])
    ctx.restoreGState()

    // The frame: light above the slanted cut, violet below, a gap between.
    let frame = CGRect(x: 262, y: 262, width: 500, height: 500)
    let gap: CGFloat = 26
    func half(_ upper: Bool) -> CGPath {
        let p = CGMutablePath()
        // The cut runs from 62% up the left edge to 38% up the right.
        let (yl, yr) = (frame.minY + frame.height * 0.62, frame.minY + frame.height * 0.38)
        let d = upper ? gap / 2 : -gap / 2
        if upper {
            p.move(to: CGPoint(x: frame.minX, y: yl + d))
            p.addLine(to: CGPoint(x: frame.maxX, y: yr + d))
            p.addLine(to: CGPoint(x: frame.maxX, y: frame.maxY))
            p.addLine(to: CGPoint(x: frame.minX, y: frame.maxY))
        } else {
            p.move(to: CGPoint(x: frame.minX, y: frame.minY))
            p.addLine(to: CGPoint(x: frame.maxX, y: frame.minY))
            p.addLine(to: CGPoint(x: frame.maxX, y: yr + d))
            p.addLine(to: CGPoint(x: frame.minX, y: yl + d))
        }
        p.closeSubpath()
        return p
    }
    for upper in [true, false] {
        ctx.saveGState()
        ctx.addPath(CGPath(roundedRect: frame, cornerWidth: 56, cornerHeight: 56, transform: nil))
        ctx.clip()
        ctx.addPath(half(upper))
        ctx.setFillColor(upper ? color(0xe8e9eb) : color(0xa98cff))
        ctx.fillPath()
        ctx.restoreGState()
    }
    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let dir = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "assets")
try! render(1024).representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("icon.png"))
try! render(256).representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("icon-256.png"))
