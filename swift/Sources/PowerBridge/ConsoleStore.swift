import AppKit
import SwiftUI
import PPCXKit

/// Thread-safe console buffer backed by an NSTextView (fast for large outputs).
final class ConsoleStore {
    weak var textView: NSTextView?
    private var pending: [(EventKind, Data)] = []
    private let lock = NSLock()
    private var scheduled = false
    private let maxChars = 2_000_000

    /// Callable from any thread.
    func enqueue(_ kind: EventKind, _ data: Data) {
        lock.lock()
        pending.append((kind, data))
        let needSchedule = !scheduled
        scheduled = true
        lock.unlock()
        if needSchedule {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { [weak self] in self?.flushNow() }
        }
    }

    /// Main thread only.
    func flushNow() {
        lock.lock()
        let items = pending
        pending.removeAll()
        scheduled = false
        lock.unlock()
        guard !items.isEmpty else { return }

        // Merge adjacent chunks of the same kind so multi-byte UTF-8 split across writes decodes correctly.
        var merged: [(EventKind, Data)] = []
        for (kind, data) in items {
            if let last = merged.last, last.0 == kind {
                merged[merged.count - 1].1.append(data)
            } else {
                merged.append((kind, data))
            }
        }
        let out = NSMutableAttributedString()
        for (kind, data) in merged {
            var text = String(decoding: data, as: UTF8.self)
            if kind == .log { text += "\n" }
            out.append(NSAttributedString(string: text, attributes: attributes(for: kind)))
        }
        append(out)
    }

    func appendStatus(_ text: String) {
        let s = NSAttributedString(string: "\n[\(text)]\n", attributes: [
            .font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular),
            .foregroundColor: NSColor.secondaryLabelColor,
        ])
        append(s)
    }

    func clear() {
        lock.lock()
        pending.removeAll()
        lock.unlock()
        textView?.string = ""
    }

    private func attributes(for kind: EventKind) -> [NSAttributedString.Key: Any] {
        let color: NSColor
        switch kind {
        case .stdout: color = .labelColor
        case .stderr: color = .systemRed
        case .log: color = .systemBlue
        }
        return [.font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular), .foregroundColor: color]
    }

    private func append(_ s: NSAttributedString) {
        guard let tv = textView, let storage = tv.textStorage else { return }
        let atBottom = tv.visibleRect.maxY >= tv.bounds.maxY - 24
        storage.append(s)
        if storage.length > maxChars {
            storage.deleteCharacters(in: NSRange(location: 0, length: storage.length - maxChars))
        }
        if atBottom { tv.scrollToEndOfDocument(nil) }
    }
}

struct ConsoleView: NSViewRepresentable {
    let store: ConsoleStore

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        if let tv = scroll.documentView as? NSTextView {
            tv.isEditable = false
            tv.isSelectable = true
            tv.drawsBackground = true
            tv.backgroundColor = .textBackgroundColor
            tv.textContainerInset = NSSize(width: 6, height: 6)
            tv.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
            store.textView = tv
        }
        return scroll
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {}
}
