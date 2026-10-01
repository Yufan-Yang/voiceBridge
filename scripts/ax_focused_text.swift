// Prints the text of the focused UI element of a process (via Accessibility).
// Usage: swift scripts/ax_focused_text.swift <pid>
import ApplicationServices
import Foundation

guard CommandLine.arguments.count == 2, let pid = Int32(CommandLine.arguments[1]) else {
    FileHandle.standardError.write("usage: ax_focused_text.swift <pid>\n".data(using: .utf8)!)
    exit(2)
}
let app = AXUIElementCreateApplication(pid)
var focused: CFTypeRef?
guard AXUIElementCopyAttributeValue(app, "AXFocusedUIElement" as CFString, &focused) == .success, let element = focused else {
    FileHandle.standardError.write("no focused element\n".data(using: .utf8)!)
    exit(1)
}
var value: CFTypeRef?
guard AXUIElementCopyAttributeValue(element as! AXUIElement, "AXValue" as CFString, &value) == .success else {
    FileHandle.standardError.write("focused element has no value\n".data(using: .utf8)!)
    exit(1)
}
print(value as? String ?? "", terminator: "")
