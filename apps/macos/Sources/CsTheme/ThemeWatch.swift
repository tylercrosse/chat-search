import Darwin
import Dispatch
import Foundation

/// The theme file, watched, so that dialling in the look is edit and *see*.
///
/// `chat-search-me9.8.10` loaded the file at launch and stopped there on purpose: watching it
/// "would make it edit and see … a nice second step and a different set of problems". Those
/// problems are this file, and there are two of them.
///
/// ## An editor does not save once, and does not save the same way twice
///
/// A save is two or three filesystem operations and which ones depend on the editor. **In place** —
/// truncate the file and write it again — keeps the inode and passes through a moment where the
/// file is empty or half a palette. **Atomically** — write a temp file and `rename` it over — never
/// shows a partial file at all, and replaces the inode. That second one is what silently kills a
/// watch registered on the file: the descriptor stays open on an unlinked inode nobody will ever
/// write to again, so the theme reloads once and then never again. vim does it by default on macOS,
/// which makes "watch the file" a watch that works exactly one time.
///
/// So both are watched. The **directory** hears entries appear, vanish and get renamed, which is
/// every atomic save; the **file** hears writes to an inode already in place, which a directory
/// event does not report. The file's descriptor is re-opened after every settled read, because by
/// then it may be pointing at the file the editor moved aside.
///
/// ## Events are not edits
///
/// Every one of those operations fires, and the editor's own scratch files fire the directory
/// alongside them — vim alone writes a backup, a swap file and a probe file into the same
/// directory on a single save. So events are coalesced into one read after a quiet period, and a
/// read that finds the file exactly as it was left is not a reload.
///
/// **The quiet period is not where the safety is.** No interval is long enough for every editor
/// and short enough to feel live, so what makes a half-written file harmless is the caller's rule
/// rather than this timer: what is drawn does not change until a whole file loads
/// (`docs/DECISIONS.md` ADR 25, and `ThemeReload` is where that is spent). The timer only decides
/// how often the question gets asked, and a read that fails is asked once more before it is
/// believed — the answer costs nothing and a complaint about a file that turns out to be fine is
/// the one thing this can say that is not true.
@MainActor
public final class ThemeWatch {
    /// The file, and the directory it sits in, which is the pair that survives an atomic save.
    private let url: URL
    private let quiet: Duration
    private let arrived: (Result<Theme, ThemeFile.Unreadable>) -> Void

    private var directory: (any DispatchSourceFileSystemObject)?
    private var file: (any DispatchSourceFileSystemObject)?
    private var settling: Task<Void, Never>?
    /// What the last look saw, so that a directory event about somebody else's file is not a
    /// reload. `nil` is a file that is not there, which is a state as worth remembering as any
    /// other — it is what stops an absent file being reported absent on every event.
    private var seen: Stamp?
    /// Whether the read about to happen is the second look at a file that did not parse.
    private var retrying = false

    /// - Parameter quiet: how long the events have to stop for before the file is read. 150ms is
    ///   two orders of magnitude longer than the gap between an editor's truncate and its write
    ///   and two orders shorter than a person noticing a delay, which is the whole of the choice.
    public init(
        _ url: URL, quiet: Duration = .milliseconds(150),
        arrived: @escaping (Result<Theme, ThemeFile.Unreadable>) -> Void
    ) {
        self.url = url
        self.quiet = quiet
        self.arrived = arrived
    }

    /// Start listening. Safe on a path that does not exist yet: the directory is what hears it
    /// arrive, and a file written into place while the app is up is somebody saying draw this.
    public func start() {
        seen = Stamp(of: url.path)
        directory = source(at: url.deletingLastPathComponent().path, for: .write)
        armFile()
    }

    /// Stop listening. Nothing in the app calls this — the watch lives as long as the process —
    /// but a scripted pass that is about to delete the file it was exercising does.
    public func stop() {
        settling?.cancel()
        settling = nil
        directory?.cancel()
        directory = nil
        file?.cancel()
        file = nil
    }

    /// Whether anything is actually being listened to. False when the directory could not be
    /// opened at all, which is a path nobody can write a theme into either.
    public var isWatching: Bool { directory != nil }

    // MARK: - Hearing

    /// Something happened in there. Which of the several things a save is does not matter, because
    /// the answer to all of them is to read the file once they stop.
    private func stir() {
        settling?.cancel()
        settling = Task { [quiet] in
            guard (try? await Task.sleep(for: quiet)) != nil else { return }
            self.read()
        }
    }

    /// One settled look.
    private func read() {
        // Before anything else: whatever is at the path now may be a different inode than the one
        // the file source is registered on, and the source that is not re-armed here is the source
        // that hears nothing about the next save.
        armFile()
        let stamp = Stamp(of: url.path)
        guard retrying || stamp != seen else { return }
        seen = stamp
        switch reading() {
        case .success(let theme):
            retrying = false
            arrived(.success(theme))
        case .failure(let unreadable):
            // A read that lands between an editor's truncate and its write finds a file that is
            // not a theme, and it is not one for about a millisecond. Look again before saying so.
            guard retrying else {
                retrying = true
                stir()
                return
            }
            retrying = false
            arrived(.failure(unreadable))
        }
    }

    /// `ThemeFile.load`, as a value rather than as a throw.
    ///
    /// The fallback is for an error `load` does not throw. It is still the honest reading if one
    /// ever arrives: whatever happened, nothing readable came back from that path.
    private func reading() -> Result<Theme, ThemeFile.Unreadable> {
        Result { try ThemeFile.load(url) }
            .mapError { $0 as? ThemeFile.Unreadable ?? .missing(url) }
    }

    // MARK: - Listening

    /// Point the file source at whatever is at the path now.
    private func armFile() {
        file?.cancel()
        file = source(at: url.path, for: [.write, .extend, .delete, .rename, .link, .attrib])
    }

    /// A source on a path, or nil if there is nothing there to open.
    ///
    /// `O_EVTONLY` is a descriptor for watching: it does not count as a reference that keeps the
    /// file busy, which matters because the thing being watched is a file an editor is about to
    /// move, truncate and delete.
    private func source(at path: String, for mask: DispatchSource.FileSystemEvent)
        -> (any DispatchSourceFileSystemObject)?
    {
        let descriptor = open(path, O_EVTONLY)
        guard descriptor >= 0 else { return nil }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: descriptor, eventMask: mask, queue: .main)
        // Delivered on the main queue, which is what makes the assumption below a fact rather than
        // a hope. Weakly, because the source is held by this object and holds this handler.
        source.setEventHandler { [weak self] in
            MainActor.assumeIsolated { self?.stir() }
        }
        source.setCancelHandler { close(descriptor) }
        source.resume()
        return source
    }

    /// Enough of a file to tell whether it is the one already read.
    ///
    /// The inode as well as the time and the size: an atomic save replaces the file, and a
    /// replacement written in the same nanosecond at the same length is exactly the save this
    /// would otherwise decide was not one.
    private struct Stamp: Equatable {
        let inode: UInt64
        let size: Int64
        let seconds: Int
        let nanoseconds: Int

        init?(of path: String) {
            var info = stat()
            guard stat(path, &info) == 0 else { return nil }
            inode = info.st_ino
            size = info.st_size
            seconds = info.st_mtimespec.tv_sec
            nanoseconds = info.st_mtimespec.tv_nsec
        }
    }
}
