import Foundation
import Network

enum IpcError: Error {
    case timeout
    case cancelled
    case connectionClosed
    case failed(String)
}

/// Événement du flux d'abonnement (`subscribe`).
enum SubEvent: Sendable {
    case connected
    case status(StatusDTO)
}

/// Client de l'API de contrôle locale du démon `nomad`, sur socket Unix, en
/// JSON Lines. Sans état partagé mutable (socket immuable) → `Sendable`.
///
/// Deux usages : [`subscribe`] (flux d'état long, une connexion) et [`send`]
/// (commande ponctuelle, connexion jetable). Le démon accepte N connexions ; on
/// ne multiplexe donc pas, chaque commande ouvre sa propre connexion.
final class IpcClient: @unchecked Sendable {
    private let socketPath: String
    private let queue = DispatchQueue(label: "dev.nomad.ipc")

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    // MARK: Abonnement au flux d'état

    /// Ouvre une connexion, envoie `subscribe`, et diffuse les événements. Le
    /// flux se termine (`finish`) quand la connexion tombe — l'appelant décide
    /// de se reconnecter.
    func subscribe() -> AsyncStream<SubEvent> {
        AsyncStream { continuation in
            let conn = NWConnection(to: .unix(path: self.socketPath), using: .tcp)
            let buffer = LineBuffer()
            conn.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    continuation.yield(.connected)
                    self.write(RequestDTO(v: ipcVersion, id: 1, cmd: "subscribe"), on: conn)
                    self.receiveLoop(conn, buffer, continuation)
                case .failed, .cancelled:
                    continuation.finish()
                default:
                    break
                }
            }
            continuation.onTermination = { _ in conn.cancel() }
            conn.start(queue: self.queue)
        }
    }

    private func receiveLoop(
        _ conn: NWConnection,
        _ buffer: LineBuffer,
        _ continuation: AsyncStream<SubEvent>.Continuation
    ) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, isComplete, error in
            if let data, !data.isEmpty {
                for line in buffer.append(data) {
                    if let inc = try? Self.decode(line), let status = inc.status {
                        continuation.yield(.status(status))
                    }
                }
            }
            if isComplete || error != nil {
                continuation.finish()
                return
            }
            self.receiveLoop(conn, buffer, continuation)
        }
    }

    // MARK: Commande ponctuelle

    /// Ouvre une connexion, envoie une commande, lit une réponse, ferme.
    func send(
        cmd: String,
        name: String? = nil,
        node: String? = nil,
        layout: [LayoutEntryReq]? = nil
    ) async throws -> IncomingDTO {
        let request = RequestDTO(v: ipcVersion, id: 1, cmd: cmd, name: name, node: node, layout: layout)
        return try await withCheckedThrowingContinuation { continuation in
            let conn = NWConnection(to: .unix(path: self.socketPath), using: .tcp)
            let buffer = LineBuffer()
            let box = ResumeBox(continuation)
            conn.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    self.write(request, on: conn)
                    self.readOne(conn, buffer, box)
                case .failed(let err):
                    box.fail(err)
                    conn.cancel()
                case .cancelled:
                    box.fail(IpcError.cancelled)
                default:
                    break
                }
            }
            conn.start(queue: self.queue)
        }
    }

    private func readOne(_ conn: NWConnection, _ buffer: LineBuffer, _ box: ResumeBox) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, isComplete, error in
            if let data, !data.isEmpty {
                if let first = buffer.append(data).first, let inc = try? Self.decode(first) {
                    box.succeed(inc)
                    conn.cancel()
                    return
                }
            }
            if isComplete || error != nil {
                box.fail(error ?? IpcError.connectionClosed)
                conn.cancel()
                return
            }
            self.readOne(conn, buffer, box)
        }
    }

    // MARK: Bas niveau

    private func write(_ request: RequestDTO, on conn: NWConnection) {
        guard var line = try? JSONEncoder().encode(request) else { return }
        line.append(0x0A)
        conn.send(content: line, completion: .contentProcessed { _ in })
    }

    private static func decode(_ data: Data) throws -> IncomingDTO {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(IncomingDTO.self, from: data)
    }
}

/// Accumulateur d'octets qui restitue les lignes complètes (`\n`). Accédé
/// uniquement depuis la file série de la connexion.
private final class LineBuffer {
    private var data = Data()

    func append(_ new: Data) -> [Data] {
        data.append(new)
        var lines: [Data] = []
        while let nl = data.firstIndex(of: 0x0A) {
            lines.append(data.subdata(in: data.startIndex..<nl))
            data.removeSubrange(data.startIndex...nl)
        }
        return lines
    }
}

/// Garantit une reprise unique de la continuation (les handlers de connexion
/// s'exécutent en série sur la même file).
private final class ResumeBox: @unchecked Sendable {
    private var continuation: CheckedContinuation<IncomingDTO, Error>?

    init(_ continuation: CheckedContinuation<IncomingDTO, Error>) {
        self.continuation = continuation
    }

    func succeed(_ value: IncomingDTO) {
        continuation?.resume(returning: value)
        continuation = nil
    }

    func fail(_ error: Error) {
        continuation?.resume(throwing: error)
        continuation = nil
    }
}
