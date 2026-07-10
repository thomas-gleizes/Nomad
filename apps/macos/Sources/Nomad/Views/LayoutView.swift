import SwiftUI

/// Page Disposition : les écrans du plan virtuel, à l'échelle, glissables.
/// Le lâcher envoie `set_layout` au démon (qui revalide) ; côté client la vue
/// est en lecture seule (seul le serveur applique la disposition).
struct LayoutView: View {
    let model: AppModel

    var body: some View {
        Group {
            if let status = model.status, !status.layout.isEmpty {
                LayoutCanvas(model: model, status: status)
            } else {
                ContentUnavailableView(
                    "Disposition indisponible",
                    systemImage: "rectangle.dashed",
                    description: Text(model.connectionLabel)
                )
            }
        }
        .navigationTitle("Disposition")
    }
}

// MARK: - Canvas

private struct LayoutCanvas: View {
    let model: AppModel
    let status: StatusDTO

    @State private var draggingId: String?
    @State private var dragOffset: CGSize = .zero

    private var isClient: Bool { status.role == "client" }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if isClient {
                Label("Disposition gérée par le serveur", systemImage: "lock")
                    .font(.callout).foregroundStyle(.secondary)
            } else {
                HStack(alignment: .firstTextBaseline) {
                    Text("Glissez un écran pour le repositionner. Les bords qui se touchent deviennent des zones de transition du curseur.")
                        .font(.callout).foregroundStyle(.secondary)
                    Spacer()
                    Button("Aligner automatiquement") { alignRow() }
                        .controlSize(.small)
                }
            }

            GeometryReader { geo in
                let t = PlaneTransform(screens: status.layout, viewSize: geo.size)
                ZStack(alignment: .topLeading) {
                    RoundedRectangle(cornerRadius: 10)
                        .fill(.quaternary.opacity(0.35))

                    ForEach(status.layout) { screen in
                        ScreenTile(
                            screen: screen,
                            isSelf: screen.id == status.selfId,
                            isActive: status.active == screen.id
                        )
                        .frame(width: t.scaled(screen.width), height: t.scaled(screen.height))
                        .position(tileCenter(screen, t))
                        .gesture(isClient ? nil : dragGesture(screen, t))
                    }
                }
            }
            .frame(minHeight: 320)
        }
        .padding()
    }

    /// Recalcule une rangée gauche → droite (serveur/local en premier) et
    /// l'envoie en une commande `set_layout`.
    private func alignRow() {
        let ordered = status.layout.sorted { a, b in
            if a.id == status.selfId { return true }
            if b.id == status.selfId { return false }
            return a.x < b.x
        }
        var x = 0
        var entries: [LayoutEntryReq] = []
        for s in ordered {
            entries.append(LayoutEntryReq(node: s.id, x: x, y: 0))
            x += s.width
        }
        model.command(.setLayout(entries))
    }

    private func tileCenter(_ screen: ScreenGeomDTO, _ t: PlaneTransform) -> CGPoint {
        var x = t.viewX(screen.x) + t.scaled(screen.width) / 2
        var y = t.viewY(screen.y) + t.scaled(screen.height) / 2
        if draggingId == screen.id {
            x += dragOffset.width
            y += dragOffset.height
        }
        return CGPoint(x: x, y: y)
    }

    private func dragGesture(_ screen: ScreenGeomDTO, _ t: PlaneTransform) -> some Gesture {
        DragGesture()
            .onChanged { value in
                draggingId = screen.id
                dragOffset = value.translation
            }
            .onEnded { value in
                let newX = t.planeX(t.viewX(screen.x) + value.translation.width)
                let newY = t.planeY(t.viewY(screen.y) + value.translation.height)
                let (sx, sy) = snap(newX, newY, screen: screen)
                draggingId = nil
                dragOffset = .zero
                model.command(.setLayout([LayoutEntryReq(node: screen.id, x: sx, y: sy)]))
            }
    }

    /// Aimante la position lâchée : d'abord flush contre un voisin (à `THRESHOLD`
    /// près), sinon sur une grille de 8 px. Le démon revalide de toute façon ;
    /// un chevauchement rejeté laisse la tuile revenir à sa place (vue pilotée
    /// par l'état).
    private func snap(_ x: Int, _ y: Int, screen: ScreenGeomDTO) -> (Int, Int) {
        let threshold = 48
        var sx = x
        var sy = y
        var snappedX = false
        var snappedY = false

        for o in status.layout where o.id != screen.id {
            if !snappedX, abs(sx - (o.x + o.width)) < threshold {
                sx = o.x + o.width; snappedX = true // bord gauche contre le droit de o
            }
            if !snappedX, abs((sx + screen.width) - o.x) < threshold {
                sx = o.x - screen.width; snappedX = true // bord droit contre le gauche de o
            }
            if !snappedY, abs(sy - o.y) < threshold {
                sy = o.y; snappedY = true // hauts alignés
            }
            if !snappedY, abs((sy + screen.height) - o.y) < threshold {
                sy = o.y - screen.height; snappedY = true // bas contre le haut de o
            }
            if !snappedY, abs(sy - (o.y + o.height)) < threshold {
                sy = o.y + o.height; snappedY = true // haut contre le bas de o
            }
        }
        if !snappedX { sx = Int((Double(sx) / 8).rounded()) * 8 }
        if !snappedY { sy = Int((Double(sy) / 8).rounded()) * 8 }
        return (sx, sy)
    }
}

// MARK: - Tuile

private struct ScreenTile: View {
    let screen: ScreenGeomDTO
    let isSelf: Bool
    let isActive: Bool

    var body: some View {
        RoundedRectangle(cornerRadius: 6)
            .fill(Color(nsColor: .controlBackgroundColor))
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(isActive ? Color.orange : Color.secondary.opacity(0.5),
                                  lineWidth: isActive ? 2 : 1)
            )
            .overlay(
                VStack(spacing: 2) {
                    Text(screen.id.prefix(6))
                        .font(.caption.weight(.semibold).monospaced())
                    Text("\(screen.width)×\(screen.height)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                    if isSelf {
                        Text("cette machine").font(.caption2).foregroundStyle(.secondary)
                    }
                }
                .padding(4)
            )
            .shadow(color: .black.opacity(0.15), radius: 2, y: 1)
    }
}

// MARK: - Transformation plan ↔ vue

/// Convertit les coordonnées du plan virtuel (pixels, potentiellement négatives)
/// en coordonnées de la vue, à une échelle qui tient dans `viewSize`.
private struct PlaneTransform {
    let minX: CGFloat
    let minY: CGFloat
    let scale: CGFloat
    let offset: CGSize

    init(screens: [ScreenGeomDTO], viewSize: CGSize, padding: CGFloat = 28) {
        let minX = CGFloat(screens.map(\.x).min() ?? 0)
        let minY = CGFloat(screens.map(\.y).min() ?? 0)
        let maxX = CGFloat(screens.map { $0.x + $0.width }.max() ?? 1)
        let maxY = CGFloat(screens.map { $0.y + $0.height }.max() ?? 1)
        let planeW = max(maxX - minX, 1)
        let planeH = max(maxY - minY, 1)
        let availW = max(viewSize.width - 2 * padding, 1)
        let availH = max(viewSize.height - 2 * padding, 1)
        let s = min(availW / planeW, availH / planeH, 0.25) // plafonné pour ne pas surdimensionner

        self.minX = minX
        self.minY = minY
        self.scale = s
        self.offset = CGSize(
            width: (viewSize.width - planeW * s) / 2,
            height: (viewSize.height - planeH * s) / 2
        )
    }

    func scaled(_ v: Int) -> CGFloat { CGFloat(v) * scale }
    func viewX(_ planeX: Int) -> CGFloat { (CGFloat(planeX) - minX) * scale + offset.width }
    func viewY(_ planeY: Int) -> CGFloat { (CGFloat(planeY) - minY) * scale + offset.height }
    func planeX(_ viewX: CGFloat) -> Int { Int(((viewX - offset.width) / scale + minX).rounded()) }
    func planeY(_ viewY: CGFloat) -> Int { Int(((viewY - offset.height) / scale + minY).rounded()) }
}
