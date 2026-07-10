import SwiftUI

/// Page Machines : pairs connectés (rôle, OS, IP, latence) + machines connues
/// hors ligne (avec « Oublier »).
struct MachinesView: View {
    let model: AppModel

    var body: some View {
        Group {
            if let status = model.status {
                List {
                    Section("Sur le réseau") {
                        SelfRow(status: status)
                        ForEach(status.peers) { peer in
                            PeerRow(peer: peer, active: status.active == peer.id)
                        }
                    }
                    if !status.knownOffline.isEmpty {
                        Section("Hors ligne") {
                            ForEach(status.knownOffline) { known in
                                OfflineRow(known: known) { model.command(.forget(known.id)) }
                            }
                        }
                    }
                }
            } else {
                ContentUnavailableView(
                    "Service non connecté",
                    systemImage: "bolt.horizontal.circle",
                    description: Text(model.connectionLabel)
                )
            }
        }
        .navigationTitle("Machines")
    }
}

private struct SelfRow: View {
    let status: StatusDTO

    var body: some View {
        HStack(spacing: 12) {
            StatusDot(color: .green)
            VStack(alignment: .leading, spacing: 2) {
                Text("\(status.nodeName)  ").font(.body.weight(.semibold))
                    + Text("(cette machine)").font(.callout).foregroundColor(.secondary)
                Text("\(osLabel(status.os)) · \(status.screen.width)×\(status.screen.height)")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            RoleBadge(role: status.role)
        }
        .padding(.vertical, 3)
    }
}

private struct PeerRow: View {
    let peer: PeerDTO
    let active: Bool

    var body: some View {
        HStack(spacing: 12) {
            StatusDot(color: .green)
            VStack(alignment: .leading, spacing: 2) {
                Text(peer.name).font(.body.weight(.semibold))
                Text("\(osLabel(peer.os)) · \(peer.addr ?? "adresse inconnue") · \(peer.screen.width)×\(peer.screen.height)")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if active {
                Text("actif").font(.caption.weight(.semibold)).foregroundStyle(.orange)
            }
            LatencyLabel(ms: peer.latencyMs)
            RoleBadge(role: "client")
        }
        .padding(.vertical, 3)
    }
}

private struct OfflineRow: View {
    let known: KnownPeerDTO
    let onForget: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            StatusDot(color: .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(known.name).font(.body.weight(.semibold)).foregroundStyle(.secondary)
                Text("\(osLabel(known.os)) · vue \(relativeDate(known.lastSeenUnix))")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button("Oublier", role: .destructive, action: onForget)
                .buttonStyle(.bordered)
                .controlSize(.small)
        }
        .padding(.vertical, 3)
    }
}

// MARK: - Composants

private struct StatusDot: View {
    let color: Color
    var body: some View { Circle().fill(color).frame(width: 8, height: 8) }
}

private struct RoleBadge: View {
    let role: String
    var body: some View {
        Text(roleLabel(role).uppercased())
            .font(.caption2.weight(.bold))
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(role == "server" ? Color.orange.opacity(0.16) : Color.blue.opacity(0.16))
            .foregroundStyle(role == "server" ? Color.orange : Color.blue)
            .clipShape(Capsule())
    }
}

private struct LatencyLabel: View {
    let ms: Int?
    var body: some View {
        if let ms {
            Text("\(ms) ms")
                .font(.caption.monospacedDigit())
                .foregroundStyle(ms < 20 ? .green : (ms < 60 ? .primary : .orange))
        } else {
            Text("—").font(.caption).foregroundStyle(.secondary)
        }
    }
}

// MARK: - Utilitaires

func osLabel(_ os: String) -> String {
    switch os {
    case "MacOs": return "macOS"
    case "Windows": return "Windows"
    case "Linux": return "Linux"
    default: return os
    }
}

func relativeDate(_ unix: Int) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(unix))
    let formatter = RelativeDateTimeFormatter()
    formatter.locale = Locale(identifier: "fr_FR")
    formatter.unitsStyle = .short
    return formatter.localizedString(for: date, relativeTo: Date())
}
