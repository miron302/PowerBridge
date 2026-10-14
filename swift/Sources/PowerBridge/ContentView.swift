import SwiftUI
import AppKit
import UniformTypeIdentifiers
import PPCXKit

struct ContentView: View {
    @EnvironmentObject var model: RunnerModel

    var body: some View {
        HSplitView {
            ConsoleView(store: model.console)
                .frame(minWidth: 380)
            SidebarView()
                .frame(minWidth: 290, idealWidth: 330, maxWidth: 440)
        }
        .toolbar {
            ToolbarItemGroup {
                Button { model.choose() } label: { Label("Choose…", systemImage: "folder") }
                    .disabled(model.running)
                if model.running {
                    Button { model.stop() } label: { Label("Stop", systemImage: "stop.fill") }
                } else {
                    Button { model.run() } label: { Label("Run", systemImage: "play.fill") }
                        .disabled(model.url == nil || model.analysisError != nil)
                }
                Button { model.makeStandaloneApp() } label: { Label("Make Standalone App", systemImage: "shippingbox") }
                    .disabled(model.analysis?.kind != "app" || model.running)
                    .help("Create a standalone Mac app that runs this PowerPC app through PowerBridge")
            }
        }
        .safeAreaInset(edge: .bottom) {
            HStack {
                Text(model.status).lineLimit(1).truncationMode(.middle)
                Spacer()
            }
            .font(.callout)
            .padding(.horizontal, 10).padding(.vertical, 5)
            .background(.bar)
        }
        .onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
            guard let provider = providers.first else { return false }
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                if let url = url { DispatchQueue.main.async { model.setURL(url) } }
            }
            return true
        }
    }
}

struct SidebarView: View {
    @EnvironmentObject var model: RunnerModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                packageHeader
                header
                if let err = model.analysisError {
                    Label(err, systemImage: "xmark.octagon.fill")
                        .foregroundColor(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let a = model.analysis {
                    verdict(a)
                    details(a)
                }
                options
                if let r = model.registers { registersView(r) }
                if let n = model.notice {
                    Text(n).font(.callout).textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    @ViewBuilder private var packageHeader: some View {
        if let name = model.packageName {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 10) {
                    Image(systemName: "shippingbox.fill").font(.system(size: 28)).foregroundColor(.brown)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(name).font(.headline)
                        Text(model.packageItems.isEmpty ? "Installer package" :
                                "Installer package · \(model.packageItems.count) item\(model.packageItems.count == 1 ? "" : "s") found")
                            .font(.caption).foregroundColor(.secondary)
                    }
                }
                if model.packageItems.count > 1 {
                    Picker("Open", selection: Binding(get: { model.selectedItem }, set: { model.selectItem($0) })) {
                        ForEach(model.packageItems, id: \.self) { item in
                            Text(URL(fileURLWithPath: item).lastPathComponent).tag(item)
                        }
                    }
                    .disabled(model.running)
                }
                Text("PowerBridge only looks inside the package. Nothing is installed and no installer scripts are run.")
                    .font(.caption).foregroundColor(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Divider()
        }
    }

    @ViewBuilder private var header: some View {
        if let u = model.url {
            HStack(spacing: 10) {
                Image(nsImage: NSWorkspace.shared.icon(forFile: u.path))
                    .resizable().frame(width: 40, height: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text(model.analysis?.name ?? u.lastPathComponent).font(.headline)
                    Text(model.analysis.map { $0.kind == "app" ? "PowerPC app bundle" : "PowerPC executable" } ?? "")
                        .font(.caption).foregroundColor(.secondary)
                }
            }
        } else if model.packageName == nil {
            VStack(alignment: .leading, spacing: 6) {
                Image(systemName: "square.and.arrow.down.on.square").font(.largeTitle).foregroundColor(.secondary)
                Text("Drop a PowerPC app, program or installer (.pkg) here").font(.headline)
                Text("or use Choose… in the toolbar.").font(.callout).foregroundColor(.secondary)
            }
        }
    }

    private func verdict(_ a: Analysis) -> some View {
        let (color, title, icon): (Color, String, String) = {
            switch a.verdict {
            case "runs": return (.green, "Should run", "checkmark.circle.fill")
            case "partial": return (.orange, "May run partially", "exclamationmark.triangle.fill")
            default: return (.red, "Not supported", "xmark.octagon.fill")
            }
        }()
        return VStack(alignment: .leading, spacing: 6) {
            Label(title, systemImage: icon).font(.headline).foregroundColor(color)
            Text(a.verdictText).font(.callout).fixedSize(horizontal: false, vertical: true)
            if a.verdict != "runs" {
                Text("You can still press Run to see how far it gets. Enable “Trace libSystem calls” to see what it needs.")
                    .font(.caption).foregroundColor(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(color.opacity(0.12))
        .cornerRadius(8)
    }

    private func details(_ a: Analysis) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            row("Architectures", a.archs.joined(separator: ", "))
            if let v = a.version { row("Version", v) }
            if let id = a.bundleId { row("Identifier", id) }
            if let m = a.minOs { row("Min. OS", m) }
            if !a.frameworks.isEmpty { row("Frameworks", a.frameworks.joined(separator: ", ")) }
            row("Imports", "\(a.importsTotal) total, \(a.missing.count) unsupported")
            if !a.missing.isEmpty {
                DisclosureGroup("Unsupported imports") {
                    Text(a.missing.joined(separator: "\n"))
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .font(.callout)
            }
        }
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack(alignment: .top) {
            Text(k).foregroundColor(.secondary).frame(width: 96, alignment: .leading)
            Text(v).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        }
        .font(.callout)
    }

    private var options: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Options").font(.headline)
            TextField("Arguments", text: $model.args).textFieldStyle(.roundedBorder)
            HStack {
                Text("Max instructions").font(.callout)
                TextField("0 = unlimited", text: $model.maxInstructions)
                    .textFieldStyle(.roundedBorder).frame(width: 130)
            }
            Toggle("Trace syscalls", isOn: $model.traceSyscalls)
            Toggle("Trace libSystem calls", isOn: $model.traceImports)
        }
        .disabled(model.running)
    }

    private func registersView(_ r: Registers) -> some View {
        var s = String(format: "pc  %08x   lr  %08x\nctr %08x   cr  %08x\nxer %08x\n\n", r.pc, r.lr, r.ctr, r.cr, r.xer)
        for i in 0..<32 {
            s += String(format: "r%-2ld %08x", i, r.gpr[i]) + (i % 2 == 1 ? "\n" : "   ")
        }
        return VStack(alignment: .leading, spacing: 6) {
            Text("Registers at exit").font(.headline)
            Text(s).font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
        }
    }
}
