// One file or folder of a view as the system sees it. Items are identified by their path in the
// view (the root is the root container); every item is read-only.

import FileProvider
import UniformTypeIdentifiers

extension NSFileProviderItemIdentifier {
    /// The item's path in the view, as wasld knows it (`""` is the view's root).
    var path: String {
        self == .rootContainer ? "" : rawValue
    }

    init(path: String) {
        self = path.isEmpty ? .rootContainer : NSFileProviderItemIdentifier(path)
    }
}

final class Item: NSObject, NSFileProviderItem {
    let wire: WireItem

    init(_ wire: WireItem) {
        self.wire = wire
    }

    var itemIdentifier: NSFileProviderItemIdentifier { .init(path: wire.id) }

    var parentItemIdentifier: NSFileProviderItemIdentifier { .init(path: wire.parent) }

    var filename: String { wire.id.isEmpty ? "Wasl" : wire.name }

    var contentType: UTType {
        wire.is_dir ? .folder : UTType(filenameExtension: (wire.name as NSString).pathExtension) ?? .data
    }

    /// Read-only: Finder offers no way to change, move or delete anything here.
    var capabilities: NSFileProviderItemCapabilities {
        wire.is_dir ? [.allowsReading, .allowsContentEnumerating] : [.allowsReading]
    }

    var documentSize: NSNumber? { wire.is_dir ? nil : NSNumber(value: wire.size) }

    var contentModificationDate: Date? { Date(timeIntervalSince1970: Double(wire.mtime_ms) / 1000) }

    var creationDate: Date? { contentModificationDate }

    /// The content version is Drive's (wasl-vfs `FileRef::version`); the metadata version changes
    /// with anything else the item shows, the pin included.
    var itemVersion: NSFileProviderItemVersion {
        NSFileProviderItemVersion(
            contentVersion: Data(wire.version.utf8),
            metadataVersion: Data("\(wire.name)|\(wire.size)|\(wire.mtime_ms)|\(wire.pinned)".utf8))
    }

    /// A pinned file is downloaded and kept downloaded; the rest are fetched when opened.
    var contentPolicy: NSFileProviderContentPolicy {
        wire.pinned ? .downloadEagerlyAndKeepDownloaded : .inherited
    }
}
