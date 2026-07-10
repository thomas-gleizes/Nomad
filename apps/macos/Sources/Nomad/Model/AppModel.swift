import AppKit
import Observation
import os
import SwiftUI

/// État de la liaison au démon.
enum ConnectionState: Equatable {
    case connecting
    case connected
    case reconnecting
    case noDaemon
}

/// Commande de contrôle envoyée au démon.
enum DaemonCommand {
    case rename(String)
    case forceServer
    case reconnect
    case forget(String)
}

/// État applicatif observable, unique source de vérité de l'UI. Miroir de
/// l'état exposé par le démon via l'API IPC, plus l'état de connexion local.
@MainActor
@Observable
final class AppModel {
    private(set) var status: StatusDTO?
    private(set) var connection: ConnectionState = .connecting

    private let log = Logger(subsystem: "dev.nomad.app", category: "model")
    private let client: IpcClient
    private let daemon: DaemonManager
    private var loop: Task<Void, Never>?

    init() {
        let client = IpcClient(socketPath: NomadPaths.socketPath)
        self.client = client
        self.daemon = DaemonManager(client: client)
    }

    /// Démarre la supervision du démon et la boucle d'abonnement (idempotent).
    func start() {
        guard loop == nil else { return }
        loop = Task { await self.run() }
    }

    // MARK: Boucle de connexion

    private func run() async {
        await daemon.ensureRunning()
        var backoffMs: UInt64 = 200
        while !Task.isCancelled {
            connection = .connecting
            let didConnect = await consumeStream()
            if didConnect {
                backoffMs = 200 // reset après une connexion réussie
            }
            // La connexion est tombée : le démon a peut-être relancé (rename /
            // forcer serveur / reconnecter). On re-sonde avant de retenter.
            connection = daemon.didSpawn ? .reconnecting : .noDaemon
            await daemon.ensureRunning()
            try? await Task.sleep(nanoseconds: backoffMs * 1_000_000)
            backoffMs = min(backoffMs * 2, 2000)
        }
    }

    /// Consomme un flux d'abonnement jusqu'à sa fin. Retourne `true` si la
    /// connexion a été établie au moins une fois.
    private func consumeStream() async -> Bool {
        var didConnect = false
        for await event in client.subscribe() {
            switch event {
            case .connected:
                didConnect = true
                connection = .connected
            case .status(let status):
                self.status = status
                connection = .connected
            }
        }
        return didConnect
    }

    // MARK: Commandes

    func command(_ command: DaemonCommand) {
        Task {
            do {
                switch command {
                case .rename(let name):
                    _ = try await client.send(cmd: "rename", name: name)
                case .forceServer:
                    _ = try await client.send(cmd: "force_server")
                case .reconnect:
                    _ = try await client.send(cmd: "reconnect")
                case .forget(let id):
                    _ = try await client.send(cmd: "forget", node: id)
                }
            } catch {
                log.error("commande échouée: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Quitte l'app, en demandant au démon de s'arrêter s'il a été lancé par nous.
    func quit() {
        Task {
            if daemon.didSpawn {
                _ = try? await withTimeout(0.5) { try await self.client.send(cmd: "quit") }
            }
            NSApp.terminate(nil)
        }
    }

    /// Dialogue natif de renommage (le menu ne peut pas présenter de champ).
    func promptRename() {
        let alert = NSAlert()
        alert.messageText = "Renommer ce nœud"
        alert.informativeText = "Le nom identifie cette machine auprès des autres."
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 220, height: 24))
        field.stringValue = status?.nodeName ?? ""
        alert.accessoryView = field
        alert.addButton(withTitle: "Renommer")
        alert.addButton(withTitle: "Annuler")
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertFirstButtonReturn {
            let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            if !name.isEmpty && name != status?.nodeName {
                command(.rename(name))
            }
        }
    }

    // MARK: Libellés dérivés

    var connectionLabel: String {
        switch connection {
        case .connecting: return "Connexion au service…"
        case .connected: return "Connecté"
        case .reconnecting: return "Reconnexion…"
        case .noDaemon: return "Service arrêté"
        }
    }

    var isServer: Bool { status?.role == "server" }
}
