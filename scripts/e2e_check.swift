// End-to-end check of the running VoiceBridge app on a real desktop:
// pin the frontmost window with the global shortcut, switch away, hold
// Push-to-Talk, and wait for the compiled prompt to be pasted into the
// pinned window. Uses the default shortcuts.
// Usage: swift scripts/e2e_check.swift <pid of the window's app> <timeout seconds>
import ApplicationServices
import Foundation

func fail(_ message: String) -> Never {
    print("FAIL: \(message)")
    exit(1)
}

func focusedText(_ pid: pid_t) -> String {
    let app = AXUIElementCreateApplication(pid)
    var el: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, "AXFocusedUIElement" as CFString, &el) == .success else { return "" }
    var value: CFTypeRef?
    AXUIElementCopyAttributeValue(el as! AXUIElement, "AXValue" as CFString, &value)
    return value as? String ?? ""
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

func key(_ code: CGKeyCode, _ flags: CGEventFlags, down: Bool) {
    let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)
    e?.flags = flags
    e?.post(tap: .cghidEventTap)
    usleep(40_000)
}

guard AXIsProcessTrusted() else { fail("terminal has no Accessibility permission") }
guard CommandLine.arguments.count == 3, let target = pid_t(CommandLine.arguments[1]), let timeout = Double(CommandLine.arguments[2]) else {
    fail("usage: e2e_check.swift <pid> <timeout>")
}
guard frontPid() == target else { fail("the window to pin is not frontmost") }
let initial = focusedText(target)

// Ctrl+Option+Shift+1: pin the foreground window to slot 1.
let pin: CGEventFlags = [.maskControl, .maskAlternate, .maskShift]
key(18, pin, down: true)
key(18, pin, down: false)
sleep(2)
guard focusedText(target) == initial else { fail("the pin shortcut leaked a keystroke into the window") }

// Put Finder in front so the injection has to activate the target itself.
let finder = Process()
finder.executableURL = URL(fileURLWithPath: "/usr/bin/open")
finder.arguments = ["-a", "Finder"]
try? finder.run()
finder.waitUntilExit()
sleep(1)
guard frontPid() != target else { fail("could not move focus away from the target") }

// Ctrl+Option+Space: hold Push-to-Talk for a moment, then release.
let ptt: CGEventFlags = [.maskControl, .maskAlternate]
key(49, ptt, down: true)
usleep(700_000)
key(49, ptt, down: false)
print("push-to-talk released; waiting for the prompt to be pasted…")

let deadline = Date().addingTimeInterval(timeout)
var text = initial
while Date() < deadline {
    text = focusedText(target)
    if text != initial { break }
    usleep(250_000)
}
guard text != initial else { fail("nothing was pasted within \(Int(timeout)) s") }
usleep(600_000)
text = focusedText(target)
print("--- pasted text ---\n\(text)\n-------------------")
guard frontPid() == target else { fail("target window is not in front after injection") }
guard !text.hasSuffix("\n") || initial.hasSuffix("\n") else { fail("Enter was pressed") }
print("OK")
