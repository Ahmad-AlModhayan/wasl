// The extension's side of wasld's File Provider socket (docs/ON_DEMAND.md §2.6): a Unix socket in
// the app group's container, one JSON value per line, opened by the same handshake as every wasld
// client (engine/crates/wasl-ipc/src/auth.rs). The socket answers File Provider requests only.

import CryptoKit
import Foundation

/// Where the app group's container is: the extension reads the group from its own Info.plist, the
/// helper (in Wasl.app/Contents/MacOS) from the extension's inside the app.
enum AppGroup {
    static let extensionName = "WaslFileProvider.appex"

    static var identifier: String? {
        if let g = Bundle.main.object(forInfoDictionaryKey: "WaslAppGroup") as? String {
            return g
        }
        guard let plugins = Bundle.main.builtInPlugInsURL,
              let appex = Bundle(url: plugins.appendingPathComponent(extensionName))
        else { return nil }
        return appex.object(forInfoDictionaryKey: "WaslAppGroup") as? String
    }

    static var container: URL? {
        identifier.flatMap { FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: $0) }
    }
}

/// What wasld says went wrong (wasl-ipc `RemoteError`).
struct RemoteError: Error, Decodable {
    let kind: String
    let message: String
}

/// One file or folder of a view (wasl-ipc `ProviderItem`).
struct WireItem: Decodable {
    let id: String
    let parent: String
    let name: String
    let is_dir: Bool
    let size: UInt64
    let mtime_ms: Int64
    let version: String
    let pinned: Bool
}

struct WireListing: Decodable {
    let anchor: String
    let items: [WireItem]
}

struct WireChanges: Decodable {
    let anchor: String
    let updated: [WireItem]
    let deleted: [String]
    let expired: Bool
}

struct WireContent: Decodable {
    let path: String
    let item: WireItem
    let offset: UInt64
    let len: UInt64
}

enum WasldError: Error {
    /// wasld isn't running, or its socket or token can't be reached.
    case unreachable(String)
    /// wasld answered with something this extension doesn't understand.
    case protocolError(String)
}

/// One connection per request: requests run concurrently (the system fetches several files at
/// once), and a connection costs a local handshake only.
enum Wasld {
    static func call<T: Decodable>(_ request: [String: Any]) throws -> T {
        guard let dir = AppGroup.container else {
            throw WasldError.unreachable("the app group's container is not available")
        }
        let conn = try Connection(socket: dir.appendingPathComponent("wasld.sock").path,
                                  token: dir.appendingPathComponent("ipc.token"))
        defer { conn.close() }
        return try conn.request(request)
    }

    static func item(view: String, id: String) throws -> WireItem {
        try call(["op": "provider_item", "view": view, "id": id])
    }

    /// The children of `parent`, or every item of the view when it is nil.
    static func list(view: String, parent: String?) throws -> WireListing {
        var r: [String: Any] = ["op": "provider_list", "view": view]
        if let parent { r["parent"] = parent }
        return try call(r)
    }

    static func changes(view: String, anchor: String) throws -> WireChanges {
        try call(["op": "provider_changes", "view": view, "anchor": anchor])
    }

    static func fetch(view: String, id: String, offset: UInt64? = nil, len: UInt64? = nil) throws -> WireContent {
        var r: [String: Any] = ["op": "provider_fetch", "view": view, "id": id]
        if let offset { r["offset"] = offset }
        if let len { r["len"] = len }
        return try call(r)
    }
}

/// An answer: `{"id":…,"ok":…}` or `{"id":…,"err":{"kind":…,"message":…}}`.
private struct Frame<V: Decodable>: Decodable {
    let ok: V?
    let err: RemoteError?
}

private final class Connection {
    private let fd: Int32
    private var buffer = Data()

    init(socket path: String, token tokenURL: URL) throws {
        guard let text = try? String(contentsOf: tokenURL, encoding: .utf8),
              let key = Data(base64Encoded: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              key.count == 32
        else { throw WasldError.unreachable("no token from wasld (is it running?)") }

        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw WasldError.unreachable(String(cString: strerror(errno))) }
        // A write to a socket wasld closed (it stopped mid-request) must fail, not raise
        // SIGPIPE, which would end the extension.
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else {
            Darwin.close(fd)
            throw WasldError.unreachable("the socket path is too long: \(path)")
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let connected = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            let why = String(cString: strerror(errno))
            Darwin.close(fd)
            throw WasldError.unreachable("wasld isn't answering: \(why)")
        }
        do {
            try handshake(key: SymmetricKey(data: key))
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    func close() {
        Darwin.close(fd)
    }

    /// Both sides prove they know the token: HMAC-SHA256(token, role ‖ 0 ‖ the other's nonce).
    private func handshake(key: SymmetricKey) throws {
        let mine = Self.nonce()
        try send(["hello": ["version": 1, "nonce": mine]])
        guard let theirs = try receive(),
              let proof = theirs["proof"] as? String,
              let nonce = theirs["nonce"] as? String,
              let given = Data(base64Encoded: proof),
              HMAC<SHA256>.isValidAuthenticationCode(given, authenticating: Self.message("wasld", mine), using: key)
        else { throw WasldError.unreachable("the socket isn't wasld's") }
        let answer = HMAC<SHA256>.authenticationCode(for: Self.message("wasl", nonce), using: key)
        try send(["proof": Data(answer).base64EncodedString()])
        guard let welcome = try receive(), welcome["protocol"] != nil else {
            throw WasldError.unreachable("wasld refused the handshake")
        }
    }

    private static func message(_ role: String, _ nonce: String) -> Data {
        var d = Data(role.utf8)
        d.append(0)
        d.append(Data(nonce.utf8))
        return d
    }

    private static func nonce() -> String {
        var b = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, b.count, &b)
        return Data(b).base64EncodedString()
    }

    func request<T: Decodable>(_ request: [String: Any]) throws -> T {
        try send(["id": 1, "request": request])
        guard let line = try receiveLine() else { throw WasldError.unreachable("wasld closed the connection") }
        let frame = try JSONDecoder().decode(Frame<T>.self, from: line)
        if let err = frame.err { throw err }
        guard let ok = frame.ok else { throw WasldError.protocolError("an answer without a result") }
        return ok
    }

    private func send(_ value: [String: Any]) throws {
        var line = try JSONSerialization.data(withJSONObject: value)
        line.append(0x0A)
        try line.withUnsafeBytes { raw in
            var sent = 0
            while sent < raw.count {
                let n = write(fd, raw.baseAddress! + sent, raw.count - sent)
                if n < 0 {
                    if errno == EINTR { continue }
                    throw WasldError.unreachable(String(cString: strerror(errno)))
                }
                sent += n
            }
        }
    }

    private func receive() throws -> [String: Any]? {
        guard let line = try receiveLine() else { return nil }
        return try JSONSerialization.jsonObject(with: line) as? [String: Any]
    }

    private func receiveLine() throws -> Data? {
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            if let nl = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex..<nl]
                buffer = Data(buffer[(nl + 1)...])
                return Data(line)
            }
            let n = read(fd, &chunk, chunk.count)
            if n < 0 {
                if errno == EINTR { continue }
                throw WasldError.unreachable(String(cString: strerror(errno)))
            }
            if n == 0 { return nil }
            buffer.append(contentsOf: chunk[0..<n])
        }
    }
}
