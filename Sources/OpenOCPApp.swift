import SwiftUI
import AppKit
import Foundation
import Darwin

struct ConnectionConfig {
    let sshIP: String
    let sshUser: String
    let firstSocksPort: Int
    let dashboardURL: String
    let chromePath: String
    let profilePath: String
    let keepTemporaryProfile: Bool
    let insecureTLS: Bool
}

struct ConnectionProfile: Codable, Identifiable, Hashable {
    let id: UUID
    var name: String
    var sshIP: String
    var sshUser: String
    var socksPort: String
    var dashboardURL: String
    var chromePath: String
    var profilePath: String
    var keepTemporaryProfile: Bool
    var insecureTLS: Bool
}

final class ConnectionProfileStore: ObservableObject {
    @Published private(set) var profiles: [ConnectionProfile] = []

    private let storageKey = "connectionProfiles.v1"

    init() {
        guard let data = UserDefaults.standard.data(forKey: storageKey),
              let saved = try? JSONDecoder().decode([ConnectionProfile].self, from: data) else {
            return
        }
        profiles = saved.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    func profile(id: String) -> ConnectionProfile? {
        guard let uuid = UUID(uuidString: id) else { return nil }
        return profiles.first { $0.id == uuid }
    }

    func save(_ profile: ConnectionProfile) {
        if let index = profiles.firstIndex(where: { $0.id == profile.id }) {
            profiles[index] = profile
        } else {
            profiles.append(profile)
        }
        profiles.sort { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
        persist()
    }

    func delete(id: String) {
        guard let uuid = UUID(uuidString: id) else { return }
        profiles.removeAll { $0.id == uuid }
        persist()
    }

    private func persist() {
        guard let data = try? JSONEncoder().encode(profiles) else { return }
        UserDefaults.standard.set(data, forKey: storageKey)
    }
}

enum OpenOCPError: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self {
        case .message(let text): return text
        }
    }
}

final class TunnelController: ObservableObject, @unchecked Sendable {
    @Published private(set) var isRunning = false
    @Published private(set) var statusText = "연결 안 됨"
    @Published private(set) var logText = ""
    @Published private(set) var lastError: String?

    private let worker = DispatchQueue(label: "OpenOCP.tunnel.worker", qos: .userInitiated)
    private let stateLock = NSLock()
    private var cancelRequested = false
    private var tunnelProcess: Process?
    private var chromeProcess: Process?
    private var controlSocket: String?
    private var target: String?
    private var temporaryProfile: String?
    private var keepTemporaryProfile = false

    func start(_ config: ConnectionConfig) {
        guard !isRunning else { return }

        stateLock.lock()
        cancelRequested = false
        stateLock.unlock()

        logText = ""
        lastError = nil
        isRunning = true
        statusText = "연결 준비 중…"

        worker.async { [weak self] in
            self?.run(config)
        }
    }

    func stop() {
        publishStatus("연결을 종료하는 중…")

        stateLock.lock()
        cancelRequested = true
        let chrome = chromeProcess
        let tunnel = tunnelProcess
        let socket = controlSocket
        let currentTarget = target
        stateLock.unlock()

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            if let socket, let currentTarget {
                self?.sendControlExit(socket: socket, target: currentTarget)
            }
            if chrome?.isRunning == true { chrome?.terminate() }
            if tunnel?.isRunning == true { tunnel?.terminate() }
        }
    }

    func dismissError() {
        lastError = nil
    }

    func stopImmediately() {
        stateLock.lock()
        cancelRequested = true
        let chrome = chromeProcess
        let tunnel = tunnelProcess
        let socket = controlSocket
        let currentTarget = target
        let profile = temporaryProfile
        let shouldKeepProfile = keepTemporaryProfile
        stateLock.unlock()

        if let socket, let currentTarget {
            sendControlExit(socket: socket, target: currentTarget)
            try? FileManager.default.removeItem(atPath: socket)
        }
        if chrome?.isRunning == true { chrome?.terminate() }
        if tunnel?.isRunning == true { tunnel?.terminate() }
        if let profile, !shouldKeepProfile {
            try? FileManager.default.removeItem(atPath: profile)
        }
    }

    private func run(_ config: ConnectionConfig) {
        var localSocket: String?
        var localProfile: String?
        var profileIsTemporary = false
        var tunnel: Process?
        var chrome: Process?
        var tunnelOutput: Pipe?
        var failureMessage: String?

        do {
            let validated = try validate(config)
            let port = try findAvailablePort(startingAt: validated.firstSocksPort)
            let chromeExecutable = try findChrome(requestedPath: validated.chromePath)
            let currentTarget = "\(validated.sshUser)@\(validated.sshIP)"
            let socket = "/tmp/openocp-\(getuid())-\(validated.sshIP.replacingOccurrences(of: ".", with: "-"))-\(port).sock"
            localSocket = socket
            try? FileManager.default.removeItem(atPath: socket)

            stateLock.lock()
            controlSocket = socket
            target = currentTarget
            keepTemporaryProfile = validated.keepTemporaryProfile
            stateLock.unlock()

            appendLog("SSH SOCKS 터널을 엽니다: \(currentTarget) → 127.0.0.1:\(port)\n")
            publishStatus("SSH 인증 대기 중…")

            let ssh = Process()
            let sshPipe = Pipe()
            tunnelOutput = sshPipe
            ssh.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
            ssh.arguments = [
                "-M", "-S", socket,
                "-o", "ControlPersist=no",
                "-o", "ExitOnForwardFailure=yes",
                "-o", "ServerAliveInterval=30",
                "-o", "ServerAliveCountMax=3",
                "-o", "StrictHostKeyChecking=accept-new",
                "-NT", "-D", "127.0.0.1:\(port)", currentTarget
            ]

            var environment = ProcessInfo.processInfo.environment
            guard let askpass = Bundle.main.path(forResource: "ssh-askpass", ofType: "sh") else {
                throw OpenOCPError.message("앱에 SSH 비밀번호 도우미가 포함되어 있지 않습니다.")
            }
            environment["SSH_ASKPASS"] = askpass
            environment["SSH_ASKPASS_REQUIRE"] = "force"
            environment["DISPLAY"] = environment["DISPLAY"] ?? ":0"
            ssh.environment = environment
            ssh.standardInput = FileHandle.nullDevice
            ssh.standardOutput = sshPipe
            ssh.standardError = sshPipe
            stream(pipe: sshPipe)

            try ssh.run()
            tunnel = ssh
            stateLock.lock()
            tunnelProcess = ssh
            stateLock.unlock()

            var ready = false
            let deadline = Date().addingTimeInterval(600)
            while Date() < deadline {
                if wasCancelled() { throw OpenOCPError.message("사용자가 연결을 취소했습니다.") }
                if !ssh.isRunning {
                    throw OpenOCPError.message("SSH 연결에 실패했습니다. IP, 사용자 이름, 비밀번호를 확인해 주세요.")
                }
                if FileManager.default.fileExists(atPath: socket), controlCheck(socket: socket, target: currentTarget) {
                    ready = true
                    break
                }
                Thread.sleep(forTimeInterval: 0.4)
            }
            guard ready else {
                throw OpenOCPError.message("SSH 인증 대기 시간이 초과되었습니다.")
            }

            appendLog("SSH 터널 연결 완료\n")
            publishStatus("대시보드 주소 확인 중…")

            let openURL: String
            if !validated.dashboardURL.isEmpty {
                openURL = validated.dashboardURL
            } else {
                let consoleURL = capture(
                    executable: "/usr/bin/ssh",
                    arguments: [
                        "-S", socket,
                        "-o", "ConnectTimeout=10",
                        currentTarget,
                        "oc whoami --show-console 2>/dev/null || true"
                    ]
                ).trimmingCharacters(in: .whitespacesAndNewlines)

                let consoleHost = capture(
                    executable: "/usr/bin/ssh",
                    arguments: [
                        "-S", socket,
                        "-o", "ConnectTimeout=10",
                        currentTarget,
                        "oc get route console -n openshift-console -o jsonpath='{.spec.host}' 2>/dev/null || true"
                    ]
                ).trimmingCharacters(in: .whitespacesAndNewlines)

                let rhoaiHost = capture(
                    executable: "/usr/bin/ssh",
                    arguments: [
                        "-S", socket,
                        "-o", "ConnectTimeout=10",
                        currentTarget,
                        "oc get route rhods-dashboard -n redhat-ods-applications -o jsonpath='{.spec.host}' 2>/dev/null || true"
                    ]
                ).trimmingCharacters(in: .whitespacesAndNewlines)

                if isValidWebURL(consoleURL) {
                    openURL = consoleURL
                    appendLog("OpenShift 콘솔 주소를 자동으로 찾았습니다.\n")
                } else if isValidHostname(consoleHost) {
                    openURL = "https://\(consoleHost)/"
                    appendLog("OpenShift console Route를 자동으로 찾았습니다.\n")
                } else if isValidHostname(rhoaiHost) {
                    openURL = "https://\(rhoaiHost)/"
                    appendLog("RHOAI Dashboard Route를 자동으로 찾았습니다.\n")
                } else {
                    openURL = "https://console-openshift-console.apps.ocp4.example.com/"
                    appendLog("콘솔 주소를 찾지 못해 현재 실습 기본 주소를 사용합니다.\n")
                }
            }

            if validated.profilePath.isEmpty {
                let base = FileManager.default.temporaryDirectory
                let profileURL = base.appendingPathComponent("RemoteOCP-Chrome-\(UUID().uuidString)", isDirectory: true)
                try FileManager.default.createDirectory(at: profileURL, withIntermediateDirectories: true)
                localProfile = profileURL.path
                profileIsTemporary = true
            } else {
                localProfile = (validated.profilePath as NSString).expandingTildeInPath
                try FileManager.default.createDirectory(atPath: localProfile!, withIntermediateDirectories: true)
            }

            if let localProfile {
                stateLock.lock()
                temporaryProfile = profileIsTemporary ? localProfile : nil
                stateLock.unlock()
            }

            guard !wasCancelled() else {
                throw OpenOCPError.message("사용자가 연결을 취소했습니다.")
            }

            appendLog("OpenShift/RHOAI 페이지 열기: \(openURL)\n")
            if validated.insecureTLS {
                appendLog("주의: 이 세션에서는 Chrome 인증서 검사가 완화됩니다.\n")
            }
            publishStatus("연결됨 · Chrome을 닫으면 자동 종료")

            let browser = Process()
            browser.executableURL = URL(fileURLWithPath: chromeExecutable)
            var browserArguments = [
                "--user-data-dir=\(localProfile!)",
                "--proxy-server=socks5://127.0.0.1:\(port)",
                "--test-type",
                "--disable-background-networking",
                "--no-first-run",
                "--no-default-browser-check",
                "--new-window"
            ]
            let isEducationDomain = URL(string: openURL)?.host?.lowercased().hasSuffix(".example.com") == true
            if validated.insecureTLS || isEducationDomain {
                browserArguments.append("--ignore-certificate-errors")
            }
            browserArguments.append(openURL)
            browser.arguments = browserArguments
            browser.standardOutput = FileHandle.nullDevice
            browser.standardError = FileHandle.nullDevice
            try browser.run()
            chrome = browser

            stateLock.lock()
            chromeProcess = browser
            stateLock.unlock()

            browser.waitUntilExit()
            appendLog("Chrome 창이 닫혔습니다.\n")
        } catch {
            let message = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            if !message.isEmpty { appendLog("오류: \(message)\n") }
            if !wasCancelled() { failureMessage = message }
        }

        tunnelOutput?.fileHandleForReading.readabilityHandler = nil
        if chrome?.isRunning == true { chrome?.terminate() }
        if let localSocket, let currentTarget = target {
            sendControlExit(socket: localSocket, target: currentTarget)
            try? FileManager.default.removeItem(atPath: localSocket)
        }
        if tunnel?.isRunning == true { tunnel?.terminate() }
        if profileIsTemporary, let localProfile, !config.keepTemporaryProfile {
            try? FileManager.default.removeItem(atPath: localProfile)
        } else if profileIsTemporary, let localProfile {
            appendLog("Chrome 임시 프로필 보관: \(localProfile)\n")
        }

        stateLock.lock()
        tunnelProcess = nil
        chromeProcess = nil
        controlSocket = nil
        target = nil
        temporaryProfile = nil
        stateLock.unlock()

        DispatchQueue.main.async { [weak self] in
            self?.isRunning = false
            self?.statusText = failureMessage == nil ? "연결 안 됨" : "연결 실패"
            self?.lastError = failureMessage
        }
    }

    private func validate(_ config: ConnectionConfig) throws -> ConnectionConfig {
        var address = in_addr()
        guard inet_pton(AF_INET, config.sshIP, &address) == 1 else {
            throw OpenOCPError.message("올바른 IPv4 주소를 입력해 주세요.")
        }
        guard !config.sshUser.isEmpty,
              config.sshUser.range(of: #"^[A-Za-z0-9._-]+$"#, options: .regularExpression) != nil else {
            throw OpenOCPError.message("SSH 사용자 이름이 올바르지 않습니다.")
        }
        guard (1024...65515).contains(config.firstSocksPort) else {
            throw OpenOCPError.message("SOCKS 포트는 1024~65515 사이여야 합니다.")
        }
        if !config.dashboardURL.isEmpty {
            guard let components = URLComponents(string: config.dashboardURL),
                  ["http", "https"].contains(components.scheme?.lowercased() ?? ""),
                  components.host != nil else {
                throw OpenOCPError.message("대시보드 URL은 http:// 또는 https://로 시작해야 합니다.")
            }
        }
        return config
    }

    private func findChrome(requestedPath: String) throws -> String {
        if !requestedPath.isEmpty {
            let expanded = (requestedPath as NSString).expandingTildeInPath
            guard FileManager.default.isExecutableFile(atPath: expanded) else {
                throw OpenOCPError.message("선택한 Chrome 실행 파일을 실행할 수 없습니다: \(expanded)")
            }
            return expanded
        }

        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
            "/Applications/Chromium.app/Contents/MacOS/Chromium"
        ]
        if let found = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) {
            return found
        }
        throw OpenOCPError.message("Google Chrome 또는 Chromium을 찾을 수 없습니다. Chrome 경로를 직접 선택해 주세요.")
    }

    private func findAvailablePort(startingAt first: Int) throws -> Int {
        for port in first..<(first + 20) where canBind(port: port) {
            return port
        }
        throw OpenOCPError.message("\(first)~\(first + 19) 범위에서 사용 가능한 SOCKS 포트를 찾지 못했습니다.")
    }

    private func canBind(port: Int) -> Bool {
        let descriptor = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard descriptor >= 0 else { return false }
        defer { Darwin.close(descriptor) }

        var value: Int32 = 1
        setsockopt(descriptor, SOL_SOCKET, SO_REUSEADDR, &value, socklen_t(MemoryLayout<Int32>.size))

        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = in_port_t(port).bigEndian
        address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))

        return withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) == 0
            }
        }
    }

    private func isValidHostname(_ text: String) -> Bool {
        !text.isEmpty && text.range(of: #"^[A-Za-z0-9.-]+$"#, options: .regularExpression) != nil
    }

    private func isValidWebURL(_ text: String) -> Bool {
        guard let components = URLComponents(string: text),
              ["http", "https"].contains(components.scheme?.lowercased() ?? ""),
              components.host != nil else {
            return false
        }
        return true
    }

    private func controlCheck(socket: String, target: String) -> Bool {
        runSilent(executable: "/usr/bin/ssh", arguments: ["-S", socket, "-O", "check", target]) == 0
    }

    private func sendControlExit(socket: String, target: String) {
        _ = runSilent(executable: "/usr/bin/ssh", arguments: ["-S", socket, "-O", "exit", target])
    }

    private func runSilent(executable: String, arguments: [String]) -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus
        } catch {
            return -1
        }
    }

    private func capture(executable: String, arguments: [String]) -> String {
        let process = Process()
        let output = Pipe()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else { return "" }
            return String(data: data, encoding: .utf8) ?? ""
        } catch {
            return ""
        }
    }

    private func stream(pipe: Pipe) {
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
            self?.appendLog(text)
        }
    }

    private func wasCancelled() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return cancelRequested
    }

    private func appendLog(_ text: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.logText.append(text)
            if self.logText.count > 80_000 {
                self.logText = String(self.logText.suffix(60_000))
            }
        }
    }

    private func publishStatus(_ text: String) {
        DispatchQueue.main.async { [weak self] in
            self?.statusText = text
        }
    }
}

struct ContentView: View {
    @ObservedObject var controller: TunnelController
    @StateObject private var profileStore = ConnectionProfileStore()

    @AppStorage("sshIP") private var sshIP = ""
    @AppStorage("sshUser") private var sshUser = "student"
    @AppStorage("socksPort") private var socksPort = "1080"
    @AppStorage("dashboardURL") private var dashboardURL = ""
    @AppStorage("chromePath") private var chromePath = ""
    @AppStorage("profilePath") private var profilePath = ""
    @AppStorage("keepTemporaryProfile") private var keepTemporaryProfile = false
    @AppStorage("insecureTLS") private var insecureTLS = false
    @AppStorage("selectedConnectionProfileID") private var selectedProfileID = ""

    @State private var showAdvanced = false
    @State private var showLogs = false
    @State private var showDeleteConfirmation = false
    @State private var profileName = ""
    @State private var profileMessage = ""

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 22)
                .padding(.top, 18)
                .padding(.bottom, 14)

            Divider()

            ScrollView {
                VStack(spacing: 14) {
                    profileSection
                    connectionSection
                    advancedSection
                }
                .padding(20)
            }

            Divider()

            VStack(spacing: 12) {
                connectionBar
                logSection

                Text("SSH 키가 없으면 macOS 비밀번호 창이 나타납니다. 비밀번호는 저장하지 않습니다.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)
        }
        .frame(minWidth: 780, minHeight: 640, idealHeight: 720)
        .onAppear(perform: loadSelectedProfile)
        .alert(
            "연결할 수 없습니다",
            isPresented: Binding(
                get: { controller.lastError != nil },
                set: { if !$0 { controller.dismissError() } }
            )
        ) {
            Button("확인", role: .cancel) { controller.dismissError() }
        } message: {
            Text(controller.lastError ?? "알 수 없는 오류가 발생했습니다.")
        }
        .confirmationDialog(
            "‘\(profileStore.profile(id: selectedProfileID)?.name ?? "선택한")’ 프로필을 삭제할까요?",
            isPresented: $showDeleteConfirmation,
            titleVisibility: .visible
        ) {
            Button("프로필 삭제", role: .destructive, action: deleteProfile)
            Button("취소", role: .cancel) { }
        } message: {
            Text("삭제한 프로필은 복구할 수 없습니다. 현재 입력된 연결 정보는 화면에 남습니다.")
        }
    }

    private var header: some View {
        HStack(spacing: 14) {
            Image(nsImage: NSApplication.shared.applicationIconImage)
                .resizable()
                .scaledToFit()
                .frame(width: 58, height: 58)
                .shadow(color: .black.opacity(0.18), radius: 5, y: 2)
                .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 3) {
                Text("Remote OCP")
                    .font(.title2.weight(.semibold))
                Text("안전한 SSH 터널로 OpenShift와 RHOAI에 접속합니다")
                    .foregroundStyle(.secondary)
            }
            Spacer()

            if let profile = profileStore.profile(id: selectedProfileID) {
                Label(profile.name, systemImage: "person.crop.circle.fill")
                    .font(.callout.weight(.medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }

    private var profileSection: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 8) {
                    Picker("프로필", selection: $selectedProfileID) {
                        Text("저장된 프로필 선택").tag("")
                        ForEach(profileStore.profiles) { profile in
                            Text(profile.name).tag(profile.id.uuidString)
                        }
                    }
                    .frame(minWidth: 220)
                    .onChange(of: selectedProfileID) { newValue in
                        guard let profile = profileStore.profile(id: newValue) else { return }
                        apply(profile)
                    }

                    TextField("프로필 이름", text: $profileName)
                        .textFieldStyle(.roundedBorder)

                    Button {
                        prepareNewProfile()
                    } label: {
                        Label("새로 만들기", systemImage: "plus")
                    }
                    .help("빈 연결 프로필을 만듭니다")

                    Button {
                        saveProfile()
                    } label: {
                        Label("저장", systemImage: "square.and.arrow.down")
                    }
                    .disabled(connectionIssue != nil)

                    Button(role: .destructive) {
                        showDeleteConfirmation = true
                    } label: {
                        Image(systemName: "trash")
                    }
                    .help("선택한 프로필 삭제")
                    .disabled(profileStore.profile(id: selectedProfileID) == nil)
                }

                if !profileMessage.isEmpty {
                    Label(profileMessage, systemImage: "checkmark.circle.fill")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(4)
        } label: {
            Label("접속 프로필", systemImage: "person.crop.rectangle.stack")
        }
        .disabled(controller.isRunning)
    }

    private var connectionSection: some View {
        GroupBox {
            Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 11) {
                GridRow {
                    Label("SSH IP", systemImage: "server.rack")
                        .frame(width: 130, alignment: .leading)
                    TextField("예: 100.68.169.37", text: $sshIP)
                        .textFieldStyle(.roundedBorder)
                }
                GridRow {
                    Label("SSH 사용자", systemImage: "person")
                        .frame(width: 130, alignment: .leading)
                    TextField("student", text: $sshUser)
                        .textFieldStyle(.roundedBorder)
                }
                GridRow {
                    Label("대시보드 URL", systemImage: "safari")
                        .frame(width: 130, alignment: .leading)
                    TextField("비워두면 OpenShift 콘솔을 자동 탐색", text: $dashboardURL)
                        .textFieldStyle(.roundedBorder)
                }
            }
            .padding(5)

            if let issue = connectionIssue, !sshIP.isEmpty {
                Label(issue, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 5)
                    .padding(.top, 3)
            }
        } label: {
            Label("연결 정보", systemImage: "network")
        }
        .disabled(controller.isRunning)
    }

    private var advancedSection: some View {
        DisclosureGroup(isExpanded: $showAdvanced) {
            Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 10) {
                GridRow {
                    Text("SOCKS 시작 포트")
                        .frame(width: 140, alignment: .leading)
                    TextField("1080", text: $socksPort)
                        .textFieldStyle(.roundedBorder)
                }
                GridRow {
                    Text("Chrome 실행 파일")
                    HStack {
                        TextField("비워두면 자동 검색", text: $chromePath)
                            .textFieldStyle(.roundedBorder)
                        Button("선택…", action: chooseChrome)
                    }
                }
                GridRow {
                    Text("Chrome 프로필 폴더")
                    HStack {
                        TextField("비워두면 임시 프로필", text: $profilePath)
                            .textFieldStyle(.roundedBorder)
                        Button("선택…", action: chooseProfileFolder)
                    }
                }
                GridRow {
                    Text("")
                    VStack(alignment: .leading, spacing: 7) {
                        Toggle("자동 생성한 임시 Chrome 프로필 보관", isOn: $keepTemporaryProfile)
                        Toggle("신뢰되지 않은 교육용 인증서 허용", isOn: $insecureTLS)
                    }
                }
            }
            .padding(.top, 10)
        } label: {
            Label("고급 설정", systemImage: "gearshape")
                .font(.headline)
        }
        .padding(.horizontal, 6)
        .disabled(controller.isRunning)
    }

    private var connectionBar: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle()
                    .fill(controller.isRunning ? Color.green.opacity(0.16) : Color.secondary.opacity(0.12))
                    .frame(width: 38, height: 38)
                if controller.isRunning {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Image(systemName: controller.lastError == nil ? "network" : "exclamationmark.triangle.fill")
                        .foregroundStyle(controller.lastError == nil ? Color.secondary : Color.orange)
                }
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(controller.statusText)
                    .font(.headline)
                Text("\(sshUser.isEmpty ? "student" : sshUser)@\(sshIP.isEmpty ? "SSH IP를 입력하세요" : sshIP)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            if controller.isRunning {
                Button(role: .destructive) {
                    controller.stop()
                } label: {
                    Label("연결 종료", systemImage: "stop.fill")
                }
                .keyboardShortcut(.cancelAction)
            } else {
                Button {
                    startConnection()
                } label: {
                    Label("대시보드 열기", systemImage: "arrow.up.forward.app.fill")
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .keyboardShortcut(.defaultAction)
                .disabled(connectionIssue != nil)
            }
        }
        .padding(12)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay {
            RoundedRectangle(cornerRadius: 10)
                .stroke(Color.primary.opacity(0.08), lineWidth: 1)
        }
    }

    private var logSection: some View {
        DisclosureGroup(isExpanded: $showLogs) {
            ScrollView {
                Text(controller.logText.isEmpty ? "아직 실행 기록이 없습니다." : controller.logText)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(controller.logText.isEmpty ? .secondary : .primary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                    .padding(9)
            }
            .frame(minHeight: 110, maxHeight: 190)
            .background(Color(nsColor: .textBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 7))
            .padding(.top, 8)
        } label: {
            HStack {
                Label("상세 실행 기록", systemImage: "doc.text.magnifyingglass")
                Spacer()
                if !controller.logText.isEmpty {
                    Text("문제 해결용")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var connectionIssue: String? {
        let cleanIP = sshIP.trimmingCharacters(in: .whitespacesAndNewlines)
        var address = in_addr()
        guard !cleanIP.isEmpty, inet_pton(AF_INET, cleanIP, &address) == 1 else {
            return "올바른 SSH IPv4 주소를 입력해 주세요."
        }

        let cleanUser = sshUser.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanUser.isEmpty,
              cleanUser.range(of: #"^[A-Za-z0-9._-]+$"#, options: .regularExpression) != nil else {
            return "SSH 사용자 이름을 확인해 주세요."
        }

        guard let port = Int(socksPort.trimmingCharacters(in: .whitespacesAndNewlines)),
              (1024...65515).contains(port) else {
            return "SOCKS 포트는 1024~65515 사이여야 합니다."
        }

        let cleanURL = dashboardURL.trimmingCharacters(in: .whitespacesAndNewlines)
        if !cleanURL.isEmpty {
            guard let components = URLComponents(string: cleanURL),
                  ["http", "https"].contains(components.scheme?.lowercased() ?? ""),
                  components.host != nil else {
                return "대시보드 URL은 http:// 또는 https://로 시작해야 합니다."
            }
        }
        return nil
    }

    private func loadSelectedProfile() {
        guard let profile = profileStore.profile(id: selectedProfileID) else { return }
        apply(profile)
    }

    private func apply(_ profile: ConnectionProfile) {
        profileName = profile.name
        sshIP = profile.sshIP
        sshUser = profile.sshUser
        socksPort = profile.socksPort
        dashboardURL = profile.dashboardURL
        chromePath = profile.chromePath
        profilePath = profile.profilePath
        keepTemporaryProfile = profile.keepTemporaryProfile
        insecureTLS = profile.insecureTLS
        profileMessage = "‘\(profile.name)’ 프로필을 불러왔습니다."
    }

    private func prepareNewProfile() {
        selectedProfileID = ""
        profileName = ""
        sshIP = ""
        sshUser = "student"
        socksPort = "1080"
        dashboardURL = ""
        chromePath = ""
        profilePath = ""
        keepTemporaryProfile = false
        insecureTLS = false
        profileMessage = "새 프로필 정보를 입력한 뒤 저장해 주세요."
    }

    private func saveProfile(showConfirmation: Bool = true) {
        guard connectionIssue == nil else { return }
        let cleanIP = sshIP.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanUser = sshUser.trimmingCharacters(in: .whitespacesAndNewlines)
        let requestedName = profileName.trimmingCharacters(in: .whitespacesAndNewlines)
        let savedName = requestedName.isEmpty ? "\(cleanUser)@\(cleanIP)" : requestedName
        let id = UUID(uuidString: selectedProfileID) ?? UUID()
        let profile = ConnectionProfile(
            id: id,
            name: savedName,
            sshIP: cleanIP,
            sshUser: cleanUser,
            socksPort: socksPort.trimmingCharacters(in: .whitespacesAndNewlines),
            dashboardURL: dashboardURL.trimmingCharacters(in: .whitespacesAndNewlines),
            chromePath: chromePath.trimmingCharacters(in: .whitespacesAndNewlines),
            profilePath: profilePath.trimmingCharacters(in: .whitespacesAndNewlines),
            keepTemporaryProfile: keepTemporaryProfile,
            insecureTLS: insecureTLS
        )
        profileStore.save(profile)
        selectedProfileID = id.uuidString
        profileName = savedName
        if showConfirmation {
            profileMessage = "‘\(savedName)’ 프로필을 저장했습니다."
        }
    }

    private func deleteProfile() {
        guard let profile = profileStore.profile(id: selectedProfileID) else { return }
        profileStore.delete(id: selectedProfileID)
        selectedProfileID = ""
        profileName = ""
        profileMessage = "‘\(profile.name)’ 프로필을 삭제했습니다."
    }

    private func startConnection() {
        guard connectionIssue == nil else { return }
        if profileStore.profile(id: selectedProfileID) != nil || !profileName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            saveProfile(showConfirmation: false)
        }

        controller.start(ConnectionConfig(
            sshIP: sshIP.trimmingCharacters(in: .whitespacesAndNewlines),
            sshUser: sshUser.trimmingCharacters(in: .whitespacesAndNewlines),
            firstSocksPort: Int(socksPort.trimmingCharacters(in: .whitespacesAndNewlines))!,
            dashboardURL: dashboardURL.trimmingCharacters(in: .whitespacesAndNewlines),
            chromePath: chromePath.trimmingCharacters(in: .whitespacesAndNewlines),
            profilePath: profilePath.trimmingCharacters(in: .whitespacesAndNewlines),
            keepTemporaryProfile: keepTemporaryProfile,
            insecureTLS: insecureTLS
        ))
    }

    private func chooseChrome() {
        let panel = NSOpenPanel()
        panel.title = "Google Chrome 앱 선택"
        panel.prompt = "선택"
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        guard panel.runModal() == .OK, let selected = panel.url else { return }

        if selected.pathExtension == "app", let executable = Bundle(url: selected)?.executableURL {
            chromePath = executable.path
        } else {
            chromePath = selected.path
        }
    }

    private func chooseProfileFolder() {
        let panel = NSOpenPanel()
        panel.title = "Chrome 프로필 폴더 선택"
        panel.prompt = "선택"
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let selected = panel.url else { return }
        profilePath = selected.path
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    var onTerminate: (() -> Void)?

    func applicationWillTerminate(_ notification: Notification) {
        onTerminate?()
    }
}

@main
struct OpenOCPDashboardApp: App {
    @StateObject private var controller = TunnelController()
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        WindowGroup {
            ContentView(controller: controller)
                .onAppear {
                    appDelegate.onTerminate = { [weak controller] in
                        controller?.stopImmediately()
                    }
                }
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) { }
        }
    }
}
