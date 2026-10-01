// End-to-end check of the running VoiceBridge app on a real desktop:
// hold Push-to-Talk in the frontmost window, switch to another app while the
// models work, and wait for the compiled prompt to be pasted back into the
// window where speaking started. Uses the default shortcut.
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

// Ctrl+Option+Space: hold Push-to-Talk while the target window has focus.
// No pinning: the window in focus when speaking starts is the target.
// VB_E2E_MODE=tap: press and release to start, wait, press and release to stop.
// Otherwise hold for VB_E2E_HOLD_MS (default 700 ms).
let ptt: CGEventFlags = [.maskControl, .maskAlternate]
let env = ProcessInfo.processInfo.environment
let holdMs = UInt32(env["VB_E2E_HOLD_MS"] ?? "") ?? 700
if env["VB_E2E_MODE"] == "tap" {
    key(49, ptt, down: true)
    key(49, ptt, down: false)
    usleep(holdMs * 1000)
    guard focusedText(target) == initial else { fail("text was pasted before the second tap") }
    key(49, ptt, down: true)
    key(49, ptt, down: false)
} else {
    key(49, ptt, down: true)
    usleep(holdMs * 1000)
    key(49, ptt, down: false)
}
guard focusedText(target) == initial else { fail("the shortcut leaked a keystroke into the window") }

// Move focus away while the models work: the text must still come back here.
let finder = Process()
finder.executableURL = URL(fileURLWithPath: "/usr/bin/open")
finder.arguments = ["-a", "Finder"]
try? finder.run()
finder.waitUntilExit()
sleep(1)
guard focusedText(target) == initial else { fail("the paste happened before focus could be moved; use a longer sample") }
guard frontPid() != target else { fail("could not move focus away from the target") }
print("push-to-talk released and focus moved to Finder; waiting for the paste…")

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
