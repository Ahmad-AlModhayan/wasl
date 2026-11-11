// Wasl's File Provider extension (docs/ON_DEMAND.md §2.6). Each File Provider domain is one
// read-only view; its identifier is the view's id. Everything is asked of wasld over its socket:
// items, listings, changes and content, which wasld reads through the view's block cache and
// writes to a file in the app group's container for the system to take.

import FileProvider

/// Maps what wasld (or the connection) said to what the system acts on.
func fileProviderError(_ error: Error) -> Error {
    switch error {
    case let e as RemoteError:
        switch e.kind {
        case "not_found": return NSFileProviderError(.noSuchItem)
        // The view isn't showing: its vault is locked, or wasld lost it. Finder says so.
        case "locked", "auth": return NSFileProviderError(.notAuthenticated)
        case "network": return NSFileProviderError(.serverUnreachable)
        default:
            return NSError(domain: NSCocoaErrorDomain, code: NSFileReadUnknownError,
                           userInfo: [NSLocalizedDescriptionKey: e.message])
        }
    case WasldError.unreachable:
        return NSFileProviderError(.serverUnreachable)
    default:
        return error
    }
}

@objc(WaslFileProviderExtension)
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension, NSFileProviderPartialContentFetching {
    private let view: String

    required init(domain: NSFileProviderDomain) {
        view = domain.identifier.rawValue
        super.init()
    }

    func invalidate() {}

    /// Runs `work` off the caller's thread; the progress counts one unit.
    private func run(_ work: @escaping () -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        DispatchQueue.global(qos: .userInitiated).async {
            work()
            progress.completedUnitCount = 1
        }
        return progress
    }

    func item(for identifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest,
              completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void) -> Progress {
        if identifier == .trashContainer {
            completionHandler(nil, NSFileProviderError(.noSuchItem))
            return Progress()
        }
        let view = self.view
        return run {
            do {
                completionHandler(Item(try Wasld.item(view: view, id: identifier.path)), nil)
            } catch {
                completionHandler(nil, fileProviderError(error))
            }
        }
    }

    func fetchContents(for itemIdentifier: NSFileProviderItemIdentifier, version requestedVersion: NSFileProviderItemVersion?,
                       request: NSFileProviderRequest,
                       completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void) -> Progress {
        let view = self.view
        return run {
            do {
                let content = try Wasld.fetch(view: view, id: itemIdentifier.path)
                completionHandler(URL(fileURLWithPath: content.path), Item(content.item), nil)
            } catch {
                completionHandler(nil, nil, fileProviderError(error))
            }
        }
    }

    /// A range of a file, aligned as the system asks: what a reader of a large file needs first.
    /// wasld writes the range at its own offset in the file it hands over.
    func fetchPartialContents(for itemIdentifier: NSFileProviderItemIdentifier, version requestedVersion: NSFileProviderItemVersion,
                              request: NSFileProviderRequest, minimalRange requestedRange: NSRange, aligningTo alignment: Int,
                              options: NSFileProviderFetchContentsOptions = [],
                              completionHandler: @escaping (URL?, NSFileProviderItem?, NSRange, NSFileProviderMaterializationFlags, Error?) -> Void) -> Progress {
        let view = self.view
        return run {
            let align = UInt64(max(alignment, 1))
            let start = UInt64(requestedRange.location) / align * align
            let end = (UInt64(requestedRange.location + requestedRange.length) + align - 1) / align * align
            do {
                let content = try Wasld.fetch(view: view, id: itemIdentifier.path, offset: start, len: end - start)
                if options.contains(.strictVersioning),
                   Data(content.item.version.utf8) != requestedVersion.contentVersion {
                    try? FileManager.default.removeItem(atPath: content.path)
                    completionHandler(nil, nil, NSRange(), [], NSFileProviderError(.versionNoLongerAvailable))
                    return
                }
                let range = NSRange(location: Int(content.offset), length: Int(content.len))
                completionHandler(URL(fileURLWithPath: content.path), Item(content.item), range, [], nil)
            } catch {
                completionHandler(nil, nil, NSRange(), [], fileProviderError(error))
            }
        }
    }

    // A view is read-only: nothing made or changed here goes to Drive. What the system reports
    // anyway (a file written into the folder from Terminal) stays on this Mac, excluded from sync.

    func createItem(basedOn itemTemplate: NSFileProviderItem, fields: NSFileProviderItemFields, contents url: URL?,
                    options: NSFileProviderCreateItemOptions = [], request: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void) -> Progress {
        completionHandler(nil, [], false, NSFileProviderError(.excludedFromSync))
        return Progress()
    }

    func modifyItem(_ item: NSFileProviderItem, baseVersion version: NSFileProviderItemVersion, changedFields: NSFileProviderItemFields,
                    contents newContents: URL?, options: NSFileProviderModifyItemOptions = [], request: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void) -> Progress {
        completionHandler(nil, [], false, NSFileProviderError(.excludedFromSync))
        return Progress()
    }

    func deleteItem(identifier: NSFileProviderItemIdentifier, baseVersion version: NSFileProviderItemVersion,
                    options: NSFileProviderDeleteItemOptions = [], request: NSFileProviderRequest,
                    completionHandler: @escaping (Error?) -> Void) -> Progress {
        completionHandler(NSError(domain: NSCocoaErrorDomain, code: NSFileWriteNoPermissionError))
        return Progress()
    }

    func enumerator(for containerItemIdentifier: NSFileProviderItemIdentifier,
                    request: NSFileProviderRequest) throws -> NSFileProviderEnumerator {
        switch containerItemIdentifier {
        case .workingSet: return Enumerator(view: view, parent: nil)
        case .trashContainer: throw NSFileProviderError(.noSuchItem)
        default: return Enumerator(view: view, parent: containerItemIdentifier.path)
        }
    }
}
