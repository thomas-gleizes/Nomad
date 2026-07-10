import Foundation

/// Version du protocole de contrôle, alignée sur `nomad-ipc` (champ `v`).
let ipcVersion = 1

// MARK: - Payloads d'état (miroir de `AppStatus` côté Rust)

struct ScreenDTO: Codable, Hashable, Sendable {
    let width: Int
    let height: Int
}

struct PeerDTO: Codable, Hashable, Sendable, Identifiable {
    let id: String
    let name: String
    let os: String
    let screen: ScreenDTO
    let addr: String?
    let latencyMs: Int?
}

struct KnownPeerDTO: Codable, Hashable, Sendable, Identifiable {
    let id: String
    let name: String
    let os: String
    let lastAddr: String?
    let lastSeenUnix: Int
}

struct StatusDTO: Codable, Hashable, Sendable {
    let role: String
    let selfId: String
    let nodeName: String
    let os: String
    let screen: ScreenDTO
    let peers: [PeerDTO]
    let active: String?
    let knownOffline: [KnownPeerDTO]
    let serverAddr: String?
}

// MARK: - Trames du protocole

/// Requête envoyée au démon. Les clés sont déjà en un seul mot : pas de
/// conversion de casse nécessaire.
struct RequestDTO: Codable {
    let v: Int
    let id: Int
    let cmd: String
    var name: String?
    var node: String?
}

/// Trame reçue : réponse (avec `id`) ou événement poussé (avec `event`). On
/// décode de façon permissive et on dispatche selon les champs présents.
struct IncomingDTO: Codable {
    let v: Int?
    let id: Int?
    let ok: Bool?
    let status: StatusDTO?
    let error: String?
    let event: String?
}
