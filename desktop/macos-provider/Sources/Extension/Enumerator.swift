// Lists a folder of a view, or the whole view for the system's working set, and reports what
// changed since an anchor (wasld keeps one refresh's worth of history; older anchors list again).

import FileProvider

final class Enumerator: NSObject, NSFileProviderEnumerator {
    private let view: String
    /// The folder's path, or nil for the working set (every item of the view).
    private let parent: String?
    private static let pageSize = 500

    init(view: String, parent: String?) {
        self.view = view
        self.parent = parent
    }

    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        let (view, parent) = (self.view, self.parent)
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let listing = try Wasld.list(view: view, parent: parent)
                let start = Self.offset(of: page)
                let end = min(start + Self.pageSize, listing.items.count)
                if start < end {
                    observer.didEnumerate(listing.items[start..<end].map(Item.init))
                }
                observer.finishEnumerating(upTo: end < listing.items.count ? Self.page(end) : nil)
            } catch {
                observer.finishEnumeratingWithError(fileProviderError(error))
            }
        }
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        let view = self.view
        DispatchQueue.global(qos: .utility).async {
            do {
                let changes = try Wasld.changes(view: view, anchor: String(decoding: anchor.rawValue, as: UTF8.self))
                if changes.expired {
                    observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
                    return
                }
                if !changes.updated.isEmpty {
                    observer.didUpdate(changes.updated.map(Item.init))
                }
                if !changes.deleted.isEmpty {
                    observer.didDeleteItems(withIdentifiers: changes.deleted.map { .init(path: $0) })
                }
                observer.finishEnumeratingChanges(upTo: Self.anchor(changes.anchor), moreComing: false)
            } catch {
                observer.finishEnumeratingWithError(fileProviderError(error))
            }
        }
    }

    /// One anchor for the whole view: read from the root's listing, the smallest one at hand.
    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        let view = self.view
        DispatchQueue.global(qos: .utility).async {
            completionHandler((try? Wasld.list(view: view, parent: "")).map { Self.anchor($0.anchor) })
        }
    }

    private static func anchor(_ s: String) -> NSFileProviderSyncAnchor {
        NSFileProviderSyncAnchor(Data(s.utf8))
    }

    private static func page(_ offset: Int) -> NSFileProviderPage {
        NSFileProviderPage(Data(String(offset).utf8))
    }

    /// The system's first pages (sorted by name or date) start at 0; later ones are offsets.
    private static func offset(of page: NSFileProviderPage) -> Int {
        Int(String(decoding: page.rawValue, as: UTF8.self)) ?? 0
    }
}
