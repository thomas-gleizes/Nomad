import SwiftUI

/// Espace réservé pour les sections pas encore implémentées, indiquant l'étape
/// de la feuille de route qui les apportera.
struct PlaceholderView: View {
    let section: ConsoleSection

    var body: some View {
        ContentUnavailableView {
            Label(section.title, systemImage: section.icon)
        } description: {
            Text("Cette section arrive à l'étape \(section.plannedStep) de la feuille de route.")
        }
        .navigationTitle(section.title)
    }
}
