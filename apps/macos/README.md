# Nomad — app macOS (coquille native)

Interface native macOS de contrôle du démon `nomad` : icône de **barre de menus**
+ **fenêtre console** à barre latérale. L'app ne contient **aucune logique
métier** — elle est un client léger de l'API de contrôle (`nomad-ipc`) exposée
par le démon Rust. Elle remplace le tray `nomad-ui` sur macOS.

## Prérequis

- macOS 14+
- Le binaire du démon `nomad` compilé : `cargo build` à la racine du dépôt
  (produit `target/debug/nomad`).
- Pour construire l'app : **Xcode** (recommandé) ou les **Command Line Tools**
  (`swift` suffit à compiler ; voir la note plus bas).

## Construire / lancer

Avec les Command Line Tools :

```sh
cd apps/macos
swift build          # compile
swift run            # compile et lance l'app
```

Avec Xcode : `File > Open…` sur le dossier `apps/macos` (Xcode ouvre le
`Package.swift` comme un projet), puis Run.

### Résolution du binaire du démon

Au lancement, l'app détecte un démon déjà en cours ; sinon elle en lance un en
`--headless`. Le chemin du binaire est cherché dans l'ordre :

1. variable d'environnement `NOMAD_DAEMON_PATH` ;
2. réglage `daemonPath` (UserDefaults) ;
3. `nomad` embarqué dans le bundle (étape de distribution, plus tard) ;
4. `target/release/nomad` puis `target/debug/nomad` relatifs au dépôt (dev).

Le socket de contrôle est partagé à
`~/Library/Application Support/dev.nomad.nomad/nomad.sock` (passé explicitement
au démon via `--ipc-socket`).

## Permissions macOS

Les permissions **Accessibilité** et **Surveillance de l'entrée** sont accordées
au **démon** (`nomad`), pas à l'app. En développement, `target/debug/nomad`
change de signature à chaque build : une ré-autorisation peut être nécessaire.
Le vrai flux d'onboarding viendra à l'étape 9 (distribution).

## Note : Swift Package plutôt que projet Xcode

Le plan initial prévoyait un `.xcodeproj`. On utilise un **Swift Package**
(`Package.swift`) : il se compile avec les seules Command Line Tools
(`swift build`, sans Xcode complet), s'ouvre aussi dans Xcode, et évite un
fichier `project.pbxproj` fragile à maintenir à la main. L'app reste une vraie
app de barre de menus via `NSApplication.setActivationPolicy(.accessory)` (pas
besoin d'`Info.plist`/`LSUIElement`).

## Architecture

```
NomadApp (@main)           entrée ; MenuBarExtra + Window ; app accessoire
├── Model/
│   ├── Protocol.swift      DTOs Codable (miroir de l'état IPC)
│   ├── IpcClient.swift     NWConnection sur socket Unix, JSON-lines
│   ├── DaemonManager.swift détection / spawn / supervision du démon
│   └── AppModel.swift      @Observable : état + boucle d'abonnement/reconnexion
└── Views/
    ├── MenuBarView.swift   parité tray (rôle, nom, pairs, actions)
    ├── ConsoleView.swift   NavigationSplitView, 6 sections + carte d'état
    ├── MachinesView.swift  page Machines (réelle) : pairs + hors ligne
    └── PlaceholderView.swift  5 sections à venir (étapes 3–9)
```

L'app ouvre une connexion `subscribe` longue (flux d'état, avec reconnexion à
backoff) et une connexion jetable par commande (`rename` / `force_server` /
`reconnect` / `forget`). Voir [docs/plan/02-app-macos.md](../../docs/plan/02-app-macos.md).
