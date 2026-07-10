import Foundation
import os

/// Emplacement du socket de contrôle, partagé par l'app et le démon.
///
/// Aligné sur le défaut du démon (`~/Library/Application Support/dev.nomad.nomad/`)
/// et **passé explicitement** au démon via `--ipc-socket` pour éviter toute
/// divergence de dérivation.
enum NomadPaths {
    static let socketPath: String = {
        let base = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first!
            .appendingPathComponent("dev.nomad.nomad", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base.appendingPathComponent("nomad.sock").path
    }()
}

/// Détecte, lance et supervise le démon `nomad` en processus enfant.
final class DaemonManager: @unchecked Sendable {
    private let log = Logger(subsystem: "dev.nomad.app", category: "daemon")
    private let client: IpcClient
    private var process: Process?
    /// Vrai dès qu'on a lancé le démon (même si un relaunch a remplacé le fils).
    private(set) var didSpawn = false

    init(client: IpcClient) {
        self.client = client
    }

    /// S'assure qu'un démon répond ; en lance un sinon.
    func ensureRunning() async {
        if await isUp() { return }
        spawn()
    }

    /// Teste la présence d'un démon en interrogeant `status` (avec timeout court).
    func isUp() async -> Bool {
        do {
            _ = try await withTimeout(2.0) { try await self.client.send(cmd: "status") }
            return true
        } catch {
            return false
        }
    }

    private func spawn() {
        guard let binary = resolveBinary() else {
            log.error("binaire nomad introuvable — définir NOMAD_DAEMON_PATH")
            return
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: binary)
        process.arguments = ["--headless", "--ipc-socket", NomadPaths.socketPath]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [log] handle in
            let data = handle.availableData
            if !data.isEmpty, let text = String(data: data, encoding: .utf8) {
                log.debug("[nomad] \(text, privacy: .public)")
            }
        }
        do {
            try process.run()
            self.process = process
            self.didSpawn = true
            log.info("démon lancé: \(binary, privacy: .public)")
        } catch {
            log.error("lancement du démon impossible: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Résout le chemin du binaire du démon, par ordre de priorité.
    private func resolveBinary() -> String? {
        let fm = FileManager.default
        var candidates: [String] = []

        if let env = ProcessInfo.processInfo.environment["NOMAD_DAEMON_PATH"] {
            candidates.append(env)
        }
        if let pref = UserDefaults.standard.string(forKey: "daemonPath") {
            candidates.append(pref)
        }
        if let bundled = Bundle.main.url(forResource: "nomad", withExtension: nil) {
            candidates.append(bundled.path)
        }
        // Dépôt en développement : remonte de ce fichier source jusqu'à la racine.
        //   .../Nomad/apps/macos/Sources/Nomad/Model/DaemonManager.swift
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // Model
            .deletingLastPathComponent() // Nomad
            .deletingLastPathComponent() // Sources
            .deletingLastPathComponent() // macos
            .deletingLastPathComponent() // apps
            .deletingLastPathComponent() // racine
        candidates.append(repoRoot.appendingPathComponent("target/release/nomad").path)
        candidates.append(repoRoot.appendingPathComponent("target/debug/nomad").path)

        return candidates.first { fm.isExecutableFile(atPath: $0) }
    }
}

/// Exécute `operation` avec un délai maximal ; lève `IpcError.timeout` sinon.
func withTimeout<T: Sendable>(
    _ seconds: Double,
    _ operation: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw IpcError.timeout
        }
        defer { group.cancelAll() }
        return try await group.next()!
    }
}
