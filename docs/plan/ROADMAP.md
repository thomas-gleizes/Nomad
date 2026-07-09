# Roadmap — Console de gestion Nomad (UI native macOS)

Plan maître, en grandes lignes. Chaque étape doit recevoir son **plan détaillé**
dans `docs/plan/NN-<slug>.md` avant d'être implémentée. La maquette de référence
(fonctionnalités et organisation des pages) est la « Console de gestion » :
Disposition / Machines / Clavier / Presse-papiers / Raccourcis / Paramètres.

## Architecture cible (décision structurante)

**Le cœur Rust reste 100 % partagé entre OS.** Ce qui est natif par plateforme,
c'est uniquement la coquille UI. Concrètement :

```
┌───────────────────────────┐        ┌──────────────────────────────┐
│  nomad (démon Rust)       │  IPC   │  Nomad.app (SwiftUI, macOS)  │
│  réseau, capture, inject, │◄──────►│  fenêtre + barre de menus    │
│  edge, clip, config       │ socket │  (plus tard : app WinUI)     │
└───────────────────────────┘        └──────────────────────────────┘
```

- Le binaire `nomad` actuel devient un **démon headless pilotable** : il expose
  une API de contrôle locale (socket Unix, JSON par lignes) — état, flux
  d'événements, commandes.
- L'app macOS est un **client léger** de cette API : elle n'embarque aucune
  logique métier. Elle remplace le tray `nomad-ui` sur macOS.
- Une future app Windows native réutilise le même démon et le même protocole
  (named pipe au lieu de socket Unix). Rien du cœur n'est dupliqué.

Pourquoi pas UniFFI / lib statique dans l'app ? Le contrôle par IPC permet au
service de tourner sans l'app (launchd), survit aux crashs de l'UI, et remplace
naturellement le mécanisme actuel de `relaunch()` par de vraies commandes.

## Étapes

### Étape 1 — API de contrôle du démon (Rust, cross-OS)
La clé de voûte : tout le reste en dépend.

- Nouveau crate `nomad-ipc` : serveur socket Unix (`~/.config/nomad/nomad.sock`),
  protocole JSON-lines versionné.
- Trois surfaces : `status` (snapshot de `AppStatus` enrichi), `subscribe`
  (flux d'événements poussés à chaque génération), `command`
  (rename / force-server / reconnect / quit — implémentées au début via le
  `relaunch()` existant).
- `SharedStatus` reste la source de vérité ; l'IPC ne fait que l'exposer.
- Tests : loopback IPC (comme `nomad-net/tests/loopback.rs`).

Livrable : `nomad --headless` pilotable par `nc`/script ; tray existant intact.

### Étape 2 — Squelette de l'app macOS (SwiftUI)
- Projet Xcode dans `apps/macos/` : app SwiftUI avec `MenuBarExtra` (reprend le
  rôle du tray) + fenêtre principale à sidebar (les 6 sections, vides sauf
  Machines minimal).
- Client IPC Swift (`NWConnection` sur socket Unix + `Codable`), reconnexion
  automatique au démon.
- Cycle de vie : l'app lance le démon en processus enfant (`--headless`)
  s'il ne tourne pas déjà ; launchd viendra à l'étape 9.
- Parité avec le tray actuel : rôle, nom, pairs, écran actif, actions
  renommer / forcer serveur / reconnecter / quitter.

Livrable : sur macOS, `Nomad.app` remplace le tray `nomad-ui` (qui reste le
fallback Windows).

### Étape 3 — Page Machines
- Démon : enrichir l'état exposé — OS, IP, résolution, latence (RTT ping) par
  pair ; mémoriser les machines connues dans la config (section « hors ligne »,
  action « oublier »).
- UI : liste complète de la maquette (pastille de statut, badge Serveur/Client,
  IP/latence, actions par ligne).

### Étape 4 — Disposition 2D des écrans (la grosse étape cœur)
- `nomad-core` : `Layout` v2 — chaque écran a une **position (x, y) dans un plan
  virtuel** ; les adjacences 4 directions sont **dérivées** des rectangles qui
  se touchent (aujourd'hui : rangée horizontale codée en dur). `entry_ratio`
  calculé sur le segment de bord réellement partagé.
- `EdgeController` adapté au layout 2D (haut/bas compris) ; persistance de la
  disposition en TOML (comble le gap « TOML-configurable layout »).
- Démon : commandes `get-layout` / `set-layout`, application à chaud (sans
  relaunch), rediffusion `LayoutUpdate` aux clients.
- UI : canvas drag & drop avec aimantation des bords, barres de transition,
  double-clic pour tester ; inspecteur basique (bords actifs par écran).
- Reporté à une étape ultérieure : résistance de bord, coins protégés, profils.

### Étape 5 — Dispositions clavier
- Protocole : chaque client annonce sa disposition dans `Hello` (et la
  ré-annonce si elle change) ; le serveur annonce la sienne.
- `nomad-input` : traduction **par caractère** serveur→client entre dispositions
  (AZERTY / QWERTY US-UK / QWERTZ / BÉPO), mode positionnel en option,
  touches mortes/accents.
- UI : page Clavier — sélection par machine, aperçu visuel du clavier, bascule
  caractère/positionnel.

### Étape 6 — Presse-papiers enrichi
- Protocole : dépasser le texte seul (`Clipboard { text }`) — images, puis
  fichiers ; plafond de taille avec récupération au collage.
- Démon : historique en mémoire (jamais sur disque), exclusions
  (gestionnaires de mots de passe via les types de pasteboard marqués
  confidentiels), commande « coller sur X ».
- UI : page Presse-papiers (historique, coller sur…, réglages).

### Étape 7 — Raccourcis globaux
- Serveur : hotkeys capturés quel que soit l'écran actif — bascule directe vers
  l'écran N, retour au serveur, mode présentation (transitions gelées),
  verrouiller toutes les sessions, arrêt d'urgence de la capture.
- Config TOML + page Raccourcis dans l'app (affichage d'abord, édition ensuite).

### Étape 8 — Réseau & sécurité
- Reconnexion automatique du client quand le serveur disparaît (known gap),
  gestion des collisions d'élection.
- Appairage par code PIN + chiffrement du transport.
- Bonus : Wake-on-LAN des machines connues.

### Étape 9 — Finition & distribution macOS
- Bundle `.app` avec démon embarqué, agent launchd « lancer à la session ».
- Onboarding permissions (Accessibilité + Surveillance de l'entrée : détection
  par préflight, guidage vers Réglages Système).
- Signature, notarisation, CI release (étendre `release.yml`), mises à jour.

## Ordre et dépendances

```
1 (IPC) ──► 2 (app squelette) ──► 3 (machines) ──► 4 (layout 2D)
                                          │
                                          ├──► 5 (clavier)
                                          ├──► 6 (presse-papiers)
                                          └──► 7 (raccourcis)
4..7 ──► 8 (réseau/sécurité) ──► 9 (distribution)
```

Les étapes 5, 6, 7 sont indépendantes entre elles et peuvent être réordonnées
selon l'envie. L'étape 8 peut démarrer en parallèle dès la 3.

## Invariants à respecter à chaque étape

- **Un seul propriétaire bloquant du thread principal** du démon : en mode
  piloté par l'app, le démon est toujours `--headless` → la capture garde le
  thread principal (cf. CLAUDE.md, modèle de threads).
- Tout ce qui est OS-sensible reste derrière les types de `nomad-core` ;
  `edge.rs` / `motion.rs` restent purs et testés.
- Le protocole IPC est versionné dès le premier jour (champ `v`) : l'app et le
  démon évoluent séparément.
- Chaque étape laisse `cargo test --workspace` vert et le mode headless
  pleinement fonctionnel (Linux/Windows ne régressent pas).
