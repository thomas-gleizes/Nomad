import SwiftUI

/// Contenu du menu de la barre de menus : parité avec le tray `nomad-ui`
/// (rôle, nom, écran, pairs, actif) plus l'ouverture de la console.
struct MenuBarView: View {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        if let status = model.status {
            Text("Rôle : \(roleLabel(status.role))")
            Text("Nom : \(status.nodeName)")
            Text("Écran : \(status.screen.width)×\(status.screen.height)")

            if status.peers.isEmpty {
                Text("Pairs : aucun")
            } else {
                Text("Pairs : \(status.peers.count) \(status.peers.count > 1 ? "connectés" : "connecté")")
                ForEach(status.peers) { peer in
                    Text("   • \(peer.name)\(status.active == peer.id ? " (actif)" : "")")
                }
            }
            if status.active == status.selfId {
                Text("Contrôlé à distance")
            }
        } else {
            Text(model.connectionLabel)
        }

        Divider()
        Button("Ouvrir la console…") { openConsole() }

        Divider()
        Button("Renommer…") { model.promptRename() }
        Button("Forcer le rôle serveur") { model.command(.forceServer) }
            .disabled(model.status == nil || model.isServer)
        Button("Reconnecter") { model.command(.reconnect) }

        Divider()
        Button("Quitter") { model.quit() }
    }

    private func openConsole() {
        NSApp.setActivationPolicy(.regular)
        openWindow(id: "console")
        NSApp.activate(ignoringOtherApps: true)
    }
}

/// Libellé français d'un rôle brut (`"server"` / `"client"`).
func roleLabel(_ role: String) -> String {
    switch role {
    case "server": return "Serveur"
    case "client": return "Client"
    default: return role
    }
}
