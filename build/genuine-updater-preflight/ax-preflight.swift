import AppKit
import ApplicationServices
import CryptoKit
import Foundation

struct ResultRecord: Encodable {
    let status: String
    let reason: String?
    let pid: Int32
    let identityMatches: Bool
    let trusted: Bool
    let checkButtonCount: Int
    let checkButtonEnabled: Bool
    let checkButtonPressSupported: Bool

    enum CodingKeys: String, CodingKey {
        case status, reason, pid, trusted
        case identityMatches = "identity_matches"
        case checkButtonCount = "check_button_count"
        case checkButtonEnabled = "check_button_enabled"
        case checkButtonPressSupported = "check_button_press_supported"
    }
}

func emit(_ value: ResultRecord) -> Never {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    if let data = try? encoder.encode(value), let line = String(data: data, encoding: .utf8) {
        print(line)
    }
    exit(value.status == "ready" ? 0 : 2)
}

func sha256(_ path: URL) -> String? {
    guard let handle = try? FileHandle(forReadingFrom: path) else { return nil }
    defer { try? handle.close() }
    var hasher = SHA256()
    do {
        while let chunk = try handle.read(upToCount: 1024 * 1024), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
    } catch {
        return nil
    }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
}

func attribute(_ element: AXUIElement, _ key: String) -> AnyObject? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success else {
        return nil
    }
    return value
}

func checkButtons(_ root: AXUIElement) -> [(AXUIElement, Bool, Bool)]? {
    var stack: [(AXUIElement, Int)] = [(root, 0)]
    var buttons: [(AXUIElement, Bool, Bool)] = []
    var visited = 0
    while let (node, depth) = stack.popLast() {
        visited += 1
        if visited > 4000 || depth > 32 { return nil }
        let role = attribute(node, kAXRoleAttribute as String) as? String
        let title = attribute(node, kAXTitleAttribute as String) as? String
        if role == (kAXButtonRole as String) && title == "Check for updates" {
            let enabled = (attribute(node, kAXEnabledAttribute as String) as? Bool) == true
            var actions: CFArray?
            let hasPress = AXUIElementCopyActionNames(node, &actions) == .success
                && (actions as? [String])?.contains(kAXPressAction as String) == true
            buttons.append((node, enabled, hasPress))
        }
        if let children = attribute(node, kAXChildrenAttribute as String) as? [AXUIElement] {
            for child in children { stack.append((child, depth + 1)) }
        }
    }
    return buttons
}

guard CommandLine.arguments.count == 6,
      let pid = Int32(CommandLine.arguments[1]), pid > 0 else {
    emit(ResultRecord(status: "blocked", reason: "invalid_arguments", pid: 0,
                      identityMatches: false, trusted: false, checkButtonCount: 0,
                      checkButtonEnabled: false, checkButtonPressSupported: false))
}
let expectedBundle = CommandLine.arguments[2]
let expectedVersion = CommandLine.arguments[3]
let expectedExecutableHash = CommandLine.arguments[4].lowercased()
let expectedAppPath = URL(fileURLWithPath: CommandLine.arguments[5]).standardizedFileURL.path
guard let app = NSRunningApplication(processIdentifier: pid),
      let bundleURL = app.bundleURL?.standardizedFileURL,
      let executableURL = app.executableURL?.standardizedFileURL,
      let version = Bundle(url: bundleURL)?.infoDictionary?["CFBundleShortVersionString"] as? String else {
    emit(ResultRecord(status: "blocked", reason: "process_identity_unavailable", pid: pid,
                      identityMatches: false, trusted: false, checkButtonCount: 0,
                      checkButtonEnabled: false, checkButtonPressSupported: false))
}
let identityMatches = app.bundleIdentifier == expectedBundle
    && bundleURL.path == expectedAppPath
    && version == expectedVersion
    && sha256(executableURL) == expectedExecutableHash
guard identityMatches else {
    emit(ResultRecord(status: "blocked", reason: "process_identity_mismatch", pid: pid,
                      identityMatches: false, trusted: false, checkButtonCount: 0,
                      checkButtonEnabled: false, checkButtonPressSupported: false))
}
let trustOptions = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: false] as CFDictionary
guard AXIsProcessTrustedWithOptions(trustOptions) else {
    emit(ResultRecord(status: "blocked", reason: "accessibility_trust_missing", pid: pid,
                      identityMatches: true, trusted: false, checkButtonCount: 0,
                      checkButtonEnabled: false, checkButtonPressSupported: false))
}
let appElement = AXUIElementCreateApplication(pid)
guard let buttons = checkButtons(appElement) else {
    emit(ResultRecord(status: "blocked", reason: "ax_tree_limit", pid: pid,
                      identityMatches: true, trusted: true, checkButtonCount: 0,
                      checkButtonEnabled: false, checkButtonPressSupported: false))
}
guard buttons.count == 1 else {
    emit(ResultRecord(status: "blocked", reason: "check_button_not_unique", pid: pid,
                      identityMatches: true, trusted: true, checkButtonCount: buttons.count,
                      checkButtonEnabled: false, checkButtonPressSupported: false))
}
let enabled = buttons[0].1
let press = buttons[0].2
guard enabled && press else {
    emit(ResultRecord(status: "blocked", reason: "check_button_not_actionable", pid: pid,
                      identityMatches: true, trusted: true, checkButtonCount: 1,
                      checkButtonEnabled: enabled, checkButtonPressSupported: press))
}
emit(ResultRecord(status: "ready", reason: nil, pid: pid, identityMatches: true,
                  trusted: true, checkButtonCount: 1, checkButtonEnabled: true,
                  checkButtonPressSupported: true))
