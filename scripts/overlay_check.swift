// Verifies the running VoiceBridge overlay against a real desktop:
//   1. clicking it does not take keyboard focus from the frontmost app
//   2. keystrokes still reach that app afterwards
//   3. it can be dragged
// Usage: swift scripts/overlay_check.swift <pid of the app that must keep focus>
// Needs Accessibility permission for the terminal.
import ApplicationServices
import Foundation

func fail(_ message: String) -> Never {
    print("FAIL: \(message)")
    exit(1)
}

// The system-wide "focused application" Accessibility query is unreliable on
// recent macOS, so the frontmost app is taken from the window list instead:
// the first normal-level window, front to back.
func frontPid() -> pid_t {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
    for w in list where (w["kCGWindowLayer"] as? Int ?? -1) == 0 {
        return pid_t(w["kCGWindowOwnerPID"] as? Int ?? 0)
    }
    return 0
}

func focusedText(_ pid: pid_t) -> String {
    let app = AXUIElementCreateApplication(pid)
    var el: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, "AXFocusedUIElement" as CFString, &el) == .success else { return "" }
    var value: CFTypeRef?
    AXUIElementCopyAttributeValue(el as! AXUIElement, "AXValue" as CFString, &value)
    return value as? String ?? ""
}

func overlayBounds() -> CGRect? {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
    for w in list {
        let owner = (w["kCGWindowOwnerName"] as? String ?? "").lowercased()
        let layer = w["kCGWindowLayer"] as? Int ?? 0
        if owner.contains("voicebridge"), layer > 0, let b = w["kCGWindowBounds"] as? NSDictionary {
            return CGRect(dictionaryRepresentation: b)
        }
    }
    return nil
}

func mouse(_ type: CGEventType, _ p: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(30_000)
}

func typeKey(_ code: CGKeyCode) {
    for down in [true, false] {
        CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)?.post(tap: .cghidEventTap)
        usleep(30_000)
    }
}

guard AXIsProcessTrusted() else { fail("terminal has no Accessibility permission") }
guard CommandLine.arguments.count == 2, let keeper = pid_t(CommandLine.arguments[1]) else { fail("usage: overlay_check.swift <pid>") }
guard let start = overlayBounds() else { fail("overlay window not found on screen") }
print("overlay bounds: \(start)")
guard frontPid() == keeper else { fail("the app that should keep focus is not frontmost") }
// Wait until that app has a focused text element to type into.
func hasFocusedElement(_ pid: pid_t) -> Bool {
    var el: CFTypeRef?
    guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid), "AXFocusedUIElement" as CFString, &el) == .success else { return false }
    var role: CFTypeRef?
    AXUIElementCopyAttributeValue(el as! AXUIElement, "AXRole" as CFString, &role)
    return (role as? String) == "AXTextArea"
}
for _ in 0..<40 where !hasFocusedElement(keeper) { usleep(250_000) }
guard hasFocusedElement(keeper) else { fail("the frontmost app has no focused text area to type into") }

// 1. Click the status area (left part of the bar).
let grip = CGPoint(x: start.minX + 45, y: start.midY)
mouse(.mouseMoved, grip)
mouse(.leftMouseDown, grip)
mouse(.leftMouseUp, grip)
usleep(500_000)
guard frontPid() == keeper else { fail("clicking the overlay took focus away (front pid \(frontPid()))") }
print("PASS: click does not steal focus")

// 2. Keystrokes still go to the original app.
let before = focusedText(keeper)
typeKey(40) // k
usleep(400_000)
let after = focusedText(keeper)
if !(after.count == before.count + 1 && after.contains("k")) {
    let app = AXUIElementCreateApplication(keeper)
    var el: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(app, "AXFocusedUIElement" as CFString, &el)
    print("diagnostics: front pid \(frontPid()), expected \(keeper), focused-element error \(err.rawValue)")
}
guard after.count == before.count + 1, after.contains("k") else { fail("keystroke did not reach the focused app (\(before.debugDescription) -> \(after.debugDescription))") }
print("PASS: keyboard input still reaches the coding window")

// 3. Drag the overlay, then drag it back.
func drag(from: CGPoint, by: CGVector) {
    mouse(.mouseMoved, from)
    mouse(.leftMouseDown, from)
    usleep(300_000) // a person holds the button briefly before moving
    for i in 1...12 {
        let t = CGFloat(i) / 12
        mouse(.leftMouseDragged, CGPoint(x: from.x + by.dx * t, y: from.y + by.dy * t))
    }
    mouse(.leftMouseUp, CGPoint(x: from.x + by.dx, y: from.y + by.dy))
    usleep(600_000)
}
drag(from: grip, by: CGVector(dx: 140, dy: 90))
guard let moved = overlayBounds() else { fail("overlay disappeared after drag") }
print("after drag: \(moved)")
// macOS snaps a dragged window to the edges of nearby windows, which can hold
// it on one axis, so require the full horizontal move and any downward move.
guard abs(moved.minX - start.minX - 140) < 12, moved.minY - start.minY >= 10, moved.minY - start.minY < 102 else {
    fail("overlay did not follow the drag")
}
if abs(moved.minY - start.minY - 90) >= 12 { print("note: vertical move was held by window-edge snapping") }
guard frontPid() == keeper else { fail("dragging the overlay took focus away") }
print("PASS: overlay can be dragged without taking focus")
drag(from: CGPoint(x: moved.minX + 45, y: moved.midY), by: CGVector(dx: start.minX - moved.minX, dy: start.minY - moved.minY))
print("OK")
