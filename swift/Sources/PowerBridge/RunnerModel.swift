import SwiftUI
import AppKit
import PPCXKit

final class RunnerModel: ObservableObject {
    @Published var url: URL?
    @Published var analysis: Analysis?
    @Published var analysisError: String?
    @Published var args = ""
    @Published var maxInstructions = "0"
    @Published var traceSyscalls = false
    @Published var traceImports = false
    @Published var running = false
    @Published var status = "Choose a PowerPC app or binary, or drop one on the window."
    @Published var registers: Registers?
    @Published var notice: String?
    @Published var packageName: String?
    @Published var packageItems: [String] = []
    @Published var selectedItem: String = ""

    let console = ConsoleStore()
    private var session: Session?
    private var tempRoots: [URL] = []

    init() {
        NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { [weak self] _ in
            self?.cleanTemp()
        }
    }

    private func cleanTemp() {
        for dir in tempRoots { try? FileManager.default.removeItem(at: dir) }
        tempRoots.removeAll()
    }

    private func isPackage(_ u: URL) -> Bool {
        ["pkg", "mpkg"].contains(u.pathExtension.lowercased())
    }

    // MARK: choosing

    func choose() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true            // .app bundles are file packages, so this selects them
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.message = "Choose a PowerPC app (.app), program, or installer package (.pkg)"
        if panel.runModal() == .OK, let u = panel.url { setURL(u) }
    }

    func setURL(_ u: URL) {
        guard !running else { return }
        if isPackage(u) {
            openPackage(u)
        } else {
            cleanTemp()
            packageName = nil
            packageItems = []
            selectedItem = ""
            analyzeURL(u)
        }
    }

    /// Look inside an installer package; nothing is installed and no installer scripts are run.
    private func openPackage(_ u: URL) {
        cleanTemp()
        url = nil
        analysis = nil
        analysisError = nil
        registers = nil
        notice = nil
        packageName = u.lastPathComponent
        packageItems = []
        selectedItem = ""
        status = "Looking inside \(u.lastPathComponent)…"
        let dest = FileManager.default.temporaryDirectory.appendingPathComponent("PowerBridge-\(UUID().uuidString)")
        tempRoots.append(dest)
        let path = u.path
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = PPCXKit.extractPackage(path: path, destination: dest.path)
            DispatchQueue.main.async {
                guard let self = self, self.packageName == u.lastPathComponent else { return }
                switch result {
                case .ok(let contents):
                    let items = contents.apps + contents.binaries
                    self.packageItems = items
                    if let first = items.first {
                        self.selectItem(first)
                    } else {
                        self.analysisError = "No PowerPC apps or programs were found inside this package."
                        self.status = "Nothing to run in \(u.lastPathComponent)."
                    }
                    if !contents.warnings.isEmpty {
                        self.notice = "\(contents.warnings.count) item(s) in the package were skipped for safety."
                    }
                case .failure(let message):
                    self.analysisError = message
                    self.status = "Cannot open this package."
                }
            }
        }
    }

    func selectItem(_ path: String) {
        selectedItem = path
        analyzeURL(URL(fileURLWithPath: path))
    }

    private func analyzeURL(_ u: URL) {
        url = u
        analysis = nil
        analysisError = nil
        registers = nil
        notice = nil
        status = "Analysing \(u.lastPathComponent)…"
        let path = u.path
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = PPCXKit.analyze(path: path)
            DispatchQueue.main.async {
                guard let self = self, self.url?.path == path else { return }
                switch result {
                case .ok(let a):
                    self.analysis = a
                    self.status = "\(a.name): \(a.verdict)"
                case .failure(let message):
                    self.analysisError = message
                    self.status = "Cannot run this file."
                }
            }
        }
    }

    // MARK: running

    func run() {
        guard let u = url, !running else { return }
        running = true
        registers = nil
        notice = nil
        status = "Running…"
        console.clear()

        let s = Session()
        session = s
        let argv = args.split(separator: " ").map(String.init)
        let limit = UInt64(maxInstructions.trimmingCharacters(in: .whitespaces)) ?? 0
        let traceSys = traceSyscalls
        let traceImp = traceImports
        let console = self.console
        let path = u.path

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let started = Date()
            let result = s.run(path: path, args: argv, maxInstructions: limit,
                               traceSyscalls: traceSys, traceImports: traceImp) { kind, data in
                console.enqueue(kind, data)
            }
            let regs = s.registers()
            let seconds = Date().timeIntervalSince(started)
            DispatchQueue.main.async {
                console.flushNow()
                self?.finish(result, regs, seconds)
            }
        }
    }

    func stop() { session?.stop() }

    private func finish(_ result: RunResult, _ regs: Registers, _ seconds: TimeInterval) {
        running = false
        var text: String
        var showRegs = true
        switch result {
        case .exited(let code): text = "Exited with status \(code)"
        case .stopped: text = "Stopped"
        case .instructionLimit: text = "Stopped: instruction limit reached"
        case .crashed(let m): text = "Crashed: \(m)"
        case .failed(let m): text = "Could not start: \(m)"; showRegs = false
        }
        if showRegs {
            registers = regs
            let mips = Double(regs.instructions) / max(seconds, 1e-9) / 1e6
            text += String(format: "  (%llu instructions, %.1f MIPS)", regs.instructions, mips)
        }
        status = text
        console.appendStatus(text)
    }

    // MARK: Intel wrapper

    func makeIntelApp() {
        guard let a = analysis, a.kind == "app" else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.prompt = "Create Here"
        panel.message = "Choose where to create “\(a.name) (Intel).app”"
        guard panel.runModal() == .OK, let dest = panel.url else { return }
        guard let translator = Self.translatorPath() else {
            notice = "Cannot find the bundled ppcx translator. Rebuild with scripts/bundle.sh."
            return
        }
        status = "Creating wrapper…"
        let source = a.path
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = PPCXKit.wrapApp(source: source, destinationDir: dest.path, translatorBinary: translator)
            DispatchQueue.main.async {
                switch result {
                case .success(let path):
                    self?.notice = "Created \(path)"
                    self?.status = "Wrapper created."
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
                case .failure(let e):
                    self?.notice = "Could not create wrapper: \(e.message)"
                    self?.status = "Wrapper failed."
                }
            }
        }
    }

    private static func translatorPath() -> String? {
        if let p = Bundle.main.path(forResource: "ppcx", ofType: nil) { return p }
        if let p = ProcessInfo.processInfo.environment["PPCX_BIN"], FileManager.default.isExecutableFile(atPath: p) { return p }
        let dev = FileManager.default.currentDirectoryPath + "/target/release/ppcx"
        return FileManager.default.isExecutableFile(atPath: dev) ? dev : nil
    }
}
