import AppKit
import SwiftUI

/// Sections de la console (barre latérale). Seule `machines` a du contenu réel
/// à cette étape ; les autres sont des espaces réservés.
enum ConsoleSection: String, CaseIterable, Identifiable, Hashable {
    case layout, machines, keyboard, clipboard, shortcuts, settings

    var id: String { rawValue }

    var title: String {
        switch self {
        case .layout: return "Disposition"
        case .machines: return "Machines"
        case .keyboard: return "Clavier"
        case .clipboard: return "Presse-papiers"
        case .shortcuts: return "Raccourcis"
        case .settings: return "Paramètres"
        }
    }

    var icon: String {
        switch self {
        case .layout: return "rectangle.split.3x1"
        case .machines: return "display.2"
        case .keyboard: return "keyboard"
        case .clipboard: return "doc.on.clipboard"
        case .shortcuts: return "command"
        case .settings: return "gearshape"
        }
    }

    /// Étape de la feuille de route qui apportera la section (pour l'espace réservé).
    var plannedStep: Int {
        switch self {
        case .layout: return 4
        case .machines: return 3
        case .keyboard: return 5
        case .clipboard: return 6
        case .shortcuts: return 7
        case .settings: return 9
        }
    }
}

/// Fenêtre principale : barre latérale à 6 sections + carte d'état.
struct ConsoleView: View {
    let model: AppModel
    @State private var selection: ConsoleSection? = .machines

    var body: some View {
        NavigationSplitView {
            List(selection: $selection) {
                ForEach(ConsoleSection.allCases) { section in
                    Label(section.title, systemImage: section.icon).tag(section)
                }
            }
            .navigationSplitViewColumnWidth(min: 190, ideal: 210)
            .safeAreaInset(edge: .bottom) { StatusCard(model: model) }
        } detail: {
            switch selection ?? .machines {
            case .layout:
                LayoutView(model: model)
            case .machines:
                MachinesView(model: model)
            case let other:
                PlaceholderView(section: other)
            }
        }
        .frame(minWidth: 760, minHeight: 480)
        .onDisappear {
            // Retour au mode accessoire quand la console se ferme.
            NSApp.setActivationPolicy(.accessory)
        }
    }
}

/// Carte d'état compacte en bas de la barre latérale.
struct StatusCard: View {
    let model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 7) {
                Circle()
                    .fill(model.connection == .connected ? Color.green : Color.secondary)
                    .frame(width: 8, height: 8)
                if let status = model.status {
                    Text("\(roleLabel(status.role)) · \(status.nodeName)")
                        .font(.callout.weight(.semibold))
                } else {
                    Text(model.connectionLabel).font(.callout)
                }
            }
            if let status = model.status {
                Text("\(status.peers.count) \(status.peers.count > 1 ? "pairs" : "pair") · \(status.screen.width)×\(status.screen.height)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(.quaternary.opacity(0.4))
    }
}
