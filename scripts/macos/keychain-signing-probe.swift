// Native qualification fixture only. Every operation names the caller-supplied
// temporary Keychain; this probe never opens the default or login Keychain.
import Foundation
import Security

#if PROBE_TWO
let buildMarker = "second signed build"
#else
let buildMarker = "first signed build"
#endif

func require(_ status: OSStatus, _ phase: String) {
    guard status == errSecSuccess else {
        fputs("Keychain qualification failed during \(phase): OSStatus \(status)\n", stderr)
        exit(1)
    }
}

guard CommandLine.arguments.count == 3 else {
    fputs("Expected create/read/denied and an isolated temporary Keychain path.\n", stderr)
    exit(2)
}
let mode = CommandLine.arguments[1]
let path = CommandLine.arguments[2]
guard ["create", "read", "denied"].contains(mode) else {
    fputs("Unknown Keychain qualification operation.\n", stderr)
    exit(2)
}
guard path.hasPrefix(NSTemporaryDirectory()), path.hasSuffix("/qualification.keychain-db") else {
    fputs("Refusing a Keychain outside the temporary qualification directory.\n", stderr)
    exit(2)
}
require(SecKeychainSetUserInteractionAllowed(false), "disable interaction")
var keychain: SecKeychain?
if mode == "create" {
    let created = "".withCString { password in
        SecKeychainCreate(path, 0, password, false, nil, &keychain)
    }
    require(created, "create isolated Keychain")
} else {
    require(SecKeychainOpen(path, &keychain), "open isolated Keychain")
}
guard let isolated = keychain else { exit(2) }
// The isolated Keychain uses an empty synthetic password. No real credential is
// present and the 0700 staging directory limits access while qualification runs.
require("".withCString { SecKeychainUnlock(isolated, 0, $0, true) }, "unlock isolated Keychain")
let service = "app.teleark.signing-qualification.v1"
let account = "synthetic-fixture"
let expected = Data("teleark-synthetic-keychain-continuity-fixture".utf8)
if mode == "create" {
    let add: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: account,
        kSecValueData as String: expected,
        kSecUseKeychain as String: isolated,
    ]
    require(SecItemAdd(add as CFDictionary, nil), "create caller-owned ACL item")
}
let query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: service,
    kSecAttrAccount as String: account,
    kSecMatchSearchList as String: [isolated],
    kSecReturnData as String: true,
    kSecMatchLimit as String: kSecMatchLimitOne,
]
var result: CFTypeRef?
let status = SecItemCopyMatching(query as CFDictionary, &result)
if mode == "denied" {
    guard status == errSecInteractionNotAllowed || status == errSecAuthFailed || status == errSecNoAccessForItem else {
        fputs("Altered signing identity was not rejected as an access failure: OSStatus \(status)\n", stderr)
        exit(1)
    }
    print("PASS: altered identity denied without interaction (OSStatus \(status)).")
} else {
    require(status, "read caller-owned ACL item")
    guard let bytes = result as? Data, bytes == expected else {
        fputs("Keychain qualification payload comparison failed.\n", stderr)
        exit(1)
    }
    print("PASS: \(buildMarker) read the isolated synthetic item without interaction.")
}
