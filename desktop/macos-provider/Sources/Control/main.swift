// wasl-fileprovider: manages the File Provider domains of Wasl.app's extension for wasld, which
// runs it (engine/crates/wasld/src/views.rs, `macos`). It lives next to wasld in
// Wasl.app/Contents/MacOS: only code inside the app that holds the extension may manage its domains.
//
//   wasl-fileprovider container                  the app group's container (wasld's socket is there)
//   wasl-fileprovider list                       domain ids, one per line
//   wasl-fileprovider add --id ID --name NAME    adds the view's domain; prints its folder
//   wasl-fileprovider remove --id ID             removes it, with every local copy
//   wasl-fileprovider signal --id ID             the system reads the view's items again
//   wasl-fileprovider evict --id ID              lets go of the content of the items on stdin
//                                                (paths in the view, one per line)

import FileProvider
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func option(_ name: String) -> String {
    let args = CommandLine.arguments
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { fail("missing \(name)") }
    return args[i + 1]
}

func domain(_ id: String) async throws -> NSFileProviderDomain? {
    try await NSFileProviderManager.domains().first { $0.identifier.rawValue == id }
}

func manager(_ id: String) async throws -> NSFileProviderManager {
    guard let d = try await domain(id), let m = NSFileProviderManager(for: d) else {
        fail("no domain \(id)")
    }
    return m
}

let command = CommandLine.arguments.dropFirst().first ?? ""
do {
    switch command {
    case "container":
        guard let dir = AppGroup.container else {
            fail("no app group: is wasl-fileprovider inside Wasl.app, with WaslFileProvider.appex?")
        }
        print(dir.path)

    case "list":
        for d in try await NSFileProviderManager.domains() {
            print(d.identifier.rawValue)
        }

    case "add":
        let id = option("--id")
        let d = NSFileProviderDomain(identifier: NSFileProviderDomainIdentifier(id), displayName: option("--name"))
        try await NSFileProviderManager.add(d)
        guard let m = NSFileProviderManager(for: d) else { fail("the domain \(id) was not added") }
        print(try await m.getUserVisibleURL(for: .rootContainer).path)

    case "remove":
        let id = option("--id")
        if let d = try await domain(id) {
            _ = try await NSFileProviderManager.remove(d, mode: .removeAll)
        }

    case "signal":
        try await manager(option("--id")).signalEnumerator(for: .workingSet)

    case "evict":
        let m = try await manager(option("--id"))
        while let line = readLine() {
            guard !line.isEmpty else { continue }
            // Items never downloaded, or gone from Drive, have nothing to let go of.
            try? await m.evictItem(identifier: NSFileProviderItemIdentifier(line))
        }

    default:
        fail("usage: wasl-fileprovider container | list | add --id ID --name NAME | remove --id ID | signal --id ID | evict --id ID")
    }
} catch {
    let e = error as NSError
    fail("\(command): \(e.localizedDescription) (\(e.domain) \(e.code))")
}
