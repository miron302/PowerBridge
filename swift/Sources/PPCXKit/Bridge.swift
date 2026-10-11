import Foundation
import CPPCX

// MARK: - Analysis

public struct Analysis: Decodable, Equatable {
    public let kind: String            // "app" or "binary"
    public let path: String
    public let execPath: String
    public let name: String
    public let bundleId: String?
    public let version: String?
    public let minOs: String?
    public let archs: [String]
    public let libraries: [String]
    public let frameworks: [String]
    public let importsTotal: Int
    public let missing: [String]
    public let verdict: String         // "runs", "partial" or "unsupported"
    public let verdictText: String
}

public enum AnalysisResult {
    case ok(Analysis)
    case failure(String)
}

private struct ErrorBox: Decodable { let error: String }

/// Inspect an `.app` bundle or a PowerPC Mach-O file (does not run anything).
public func analyze(path: String) -> AnalysisResult {
    guard let c = ppcx_analyze_json(path) else { return .failure("analysis failed") }
    defer { ppcx_string_free(c) }
    let data = Data(String(cString: c).utf8)
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    if let e = try? decoder.decode(ErrorBox.self, from: data) { return .failure(e.error) }
    do {
        return .ok(try decoder.decode(Analysis.self, from: data))
    } catch {
        return .failure("unreadable analysis result: \(error)")
    }
}

// MARK: - Installer packages (.pkg / .mpkg)

public struct PackageContents: Decodable, Equatable {
    public let apps: [String]
    public let binaries: [String]
    public let files: Int
    public let bytes: UInt64
    public let warnings: [String]
}

public enum PackageResult {
    case ok(PackageContents)
    case failure(String)
}

/// Unpack an installer package into `destination` (never runs installer scripts) and list the
/// PowerPC apps and command-line programs inside.
public func extractPackage(path: String, destination: String) -> PackageResult {
    guard let c = ppcx_pkg_extract_json(path, destination) else { return .failure("could not open the package") }
    defer { ppcx_string_free(c) }
    let data = Data(String(cString: c).utf8)
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    if let e = try? decoder.decode(ErrorBox.self, from: data) { return .failure(e.error) }
    do {
        return .ok(try decoder.decode(PackageContents.self, from: data))
    } catch {
        return .failure("unreadable package result: \(error)")
    }
}

// MARK: - Running

public enum EventKind: Int32 {
    case stdout = 0
    case stderr = 1
    case log = 2
}

public enum RunResult: Equatable {
    case exited(Int32)
    case stopped
    case instructionLimit
    case crashed(String)
    case failed(String)   // could not even load the program
}

public struct Registers {
    public let gpr: [UInt32]
    public let pc: UInt32
    public let lr: UInt32
    public let ctr: UInt32
    public let cr: UInt32
    public let xer: UInt32
    public let instructions: UInt64
}

fileprivate final class CallbackBox {
    let handler: (EventKind, Data) -> Void
    init(_ handler: @escaping (EventKind, Data) -> Void) { self.handler = handler }
}

private let eventTrampoline: ppcx_event_cb = { kind, data, len, user in
    guard let user = user else { return }
    let box = Unmanaged<CallbackBox>.fromOpaque(user).takeUnretainedValue()
    let bytes: Data
    if let data = data, len > 0 {
        bytes = Data(bytes: data, count: len)
    } else {
        bytes = Data()
    }
    box.handler(EventKind(rawValue: kind) ?? .log, bytes)
}

/// One emulator session. `run` blocks, so call it from a background thread; `stop` is thread-safe.
public final class Session {
    private let handle: OpaquePointer

    public init() { handle = ppcx_session_new() }
    deinit { ppcx_session_free(handle) }

    public func stop() { ppcx_session_stop(handle) }

    private func message() -> String {
        guard let c = ppcx_session_message(handle) else { return "" }
        defer { ppcx_string_free(c) }
        return String(cString: c)
    }

    /// `path` may be an `.app` bundle or a Mach-O file. `onEvent` is called on the calling thread.
    public func run(path: String,
                    args: [String],
                    maxInstructions: UInt64,
                    traceSyscalls: Bool,
                    traceImports: Bool,
                    onEvent: @escaping (EventKind, Data) -> Void) -> RunResult {
        let box = CallbackBox(onEvent)
        let user = Unmanaged.passRetained(box).toOpaque()
        defer { Unmanaged<CallbackBox>.fromOpaque(user).release() }

        let cargs: [UnsafePointer<CChar>?] = args.map { UnsafePointer(strdup($0)!) }
        defer { cargs.forEach { free(UnsafeMutablePointer(mutating: $0)) } }

        let code = ppcx_run(handle, path, cargs, cargs.count, maxInstructions,
                            traceSyscalls ? 1 : 0, traceImports ? 1 : 0,
                            eventTrampoline, user)
        switch code {
        case 0: return .exited(ppcx_session_exit_code(handle))
        case 1: return .stopped
        case 2: return .instructionLimit
        case 3: return .crashed(message())
        default: return .failed(message())
        }
    }

    /// Final CPU state of the last run.
    public func registers() -> Registers {
        var r = [UInt32](repeating: 0, count: 40)
        ppcx_session_regs(handle, &r, 40)
        return Registers(gpr: Array(r[0..<32]), pc: r[32], lr: r[33], ctr: r[34], cr: r[35], xer: r[36],
                         instructions: (UInt64(r[37]) << 32) | UInt64(r[38]))
    }
}

// MARK: - Intel wrapper

public struct PPCXError: Error, LocalizedError {
    public let message: String
    public var errorDescription: String? { message }
}

/// Create "<Name> (Intel).app" in `destinationDir`: a normal app whose launcher runs the embedded
/// PowerPC app through the `ppcx` translator binary at `translatorBinary`. Returns the new bundle path.
public func wrapApp(source: String, destinationDir: String, translatorBinary: String) -> Result<String, PPCXError> {
    var out: UnsafeMutablePointer<CChar>? = nil
    let rc = ppcx_wrap_app(source, destinationDir, translatorBinary, &out)
    let text = out.map { String(cString: $0) } ?? ""
    if let o = out { ppcx_string_free(o) }
    return rc == 0 ? .success(text) : .failure(PPCXError(message: text))
}
