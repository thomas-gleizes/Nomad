# Étape 2 — Squelette de l'app macOS (SwiftUI)

> Plan détaillé de l'étape 2 de [ROADMAP.md](ROADMAP.md). Dépend de l'étape 1
> (`nomad-ipc`) : l'app est un **client léger** de l'API du démon, zéro logique
> métier côté Swift.
>
> **Statut : implémentée** dans [`apps/macos/`](../../apps/macos/). `MenuBarExtra`
> (parité tray), fenêtre console à barre latérale 6 sections (Machines réelle,
> reste en espaces réservés), client IPC `NWConnection`/JSON-lines avec
> reconnexion à backoff, `DaemonManager` (détection/spawn/supervision). Compile
> avec `swift build` ; l'app démarre et lance bien le démon (validé au runtime).
>
> **Écart assumé vs plan** : **Swift Package** (`Package.swift`) au lieu d'un
> `.xcodeproj`. Raison : compilable et vérifiable avec les seules Command Line
> Tools (pas de Xcode complet requis ici), s'ouvre quand même dans Xcode, et
> évite un `project.pbxproj` fragile écrit à la main. App accessoire via
> `setActivationPolicy(.accessory)` (pas d'`Info.plist`/`LSUIElement`).
>
> **Non vérifié ici** (nécessite une session GUI interactive / la permission
> Accessibilité du démon) : l'affichage live du menu et de la page Machines
> alimentés par un démon serveur qui reste en vie. La liaison app→démon (spawn,
> résolution du binaire, boucle IPC) est, elle, confirmée au runtime.

## Objectif

Une app macOS native `Nomad.app` qui :
- remplace le tray `nomad-ui` sur macOS : icône de **barre de menus**
  (`MenuBarExtra`) avec rôle, nom, pairs, écran actif + actions renommer /
  forcer serveur / reconnecter / quitter (parité stricte avec
  [tray.rs](../../crates/nomad-ui/src/tray.rs)) ;
- ouvre une **fenêtre console** à sidebar avec les 6 sections de la maquette
  (Disposition, Machines, Clavier, Presse-papiers, Raccourcis, Paramètres) —
  seule **Machines** a du contenu réel à cette étape, les autres affichent un
  état « à venir » ;
- **gère le cycle de vie du démon** : le détecte s'il tourne, le lance en
  processus enfant (`--headless`) sinon, se reconnecte après un relaunch.

## Décisions de conception

| Sujet | Décision | Pourquoi |
|---|---|---|
| Emplacement | `apps/macos/` : projet Xcode `Nomad.xcodeproj` commité + sources dans `apps/macos/Nomad/` | Sépare clairement la coquille UI du workspace Cargo ; le `.xcodeproj` commité évite un outil de génération de plus. |
| Cible minimale | macOS 14 | `MenuBarExtra` (13+) et macros `@Observable` (14+) ; aucune raison de supporter plus vieux pour un outil perso. |
| Langage/UI | Swift 5.10+, SwiftUI pur (pas d'AppKit sauf nécessité ponctuelle) | Coquille simple ; AppKit accessible plus tard pour le canvas de l'étape 4 si besoin. |
| IPC | `NWConnection` sur `NWEndpoint.unix(path:)`, framing = lignes JSON, DTOs `Codable` | Miroir exact du protocole v1 de l'étape 1. Repli si `Network.framework` se montre capricieux sur les sockets Unix : petit wrapper BSD socket + `DispatchSourceRead` (isolé derrière le même protocole Swift). |
| Sandbox | **App Sandbox désactivée** à cette étape | L'app doit spawner un binaire arbitraire et parler à un socket hors conteneur. À re-challenger à l'étape 9 (distribution). |
| Dock | `LSUIElement = YES` (app « accessoire ») | Même comportement que le tray actuel : pas d'icône Dock, la fenêtre console s'ouvre depuis la barre de menus. |
| Démon en dev | Chemin du binaire résolu dans l'ordre : ① `NOMAD_DAEMON_PATH` (env), ② réglage utilisateur (UserDefaults), ③ `Resources/nomad` embarqué (étape 9), ④ `target/{debug,release}/nomad` relatifs au repo | Confort de dev immédiat, sans attendre le bundling. |
| Arrêt | « Quitter » de la barre de menus : envoie `quit` au démon **uniquement si l'app l'a lancé**, puis quitte l'app. Si le démon préexistait, on le laisse tourner. | Principe de moindre surprise : l'app ne tue pas un service qu'elle ne possède pas. |
| Localisation | Interface en français, en dur | Cohérent avec le reste du projet (UI/docs/CLI en français). |

## Architecture Swift

```
NomadApp (@main)
├── DaemonManager        // détection / spawn / supervision du process enfant
├── IpcClient            // NWConnection + JSON-lines, async/await
│     ├── request(_:) async throws -> Response     (corrélation par id)
│     └── events: AsyncStream<StatusDTO>           (subscribe + reconnexion)
├── AppModel (@Observable)  // état miroir : ConnectionState + StatusDTO
├── MenuBarExtra            // parité tray + « Ouvrir la console… »
└── ConsoleWindow
      └── NavigationSplitView
            ├── Sidebar (6 sections + carte d'état en bas, cf. maquette)
            ├── MachinesView        // contenu réel
            └── PlaceholderView     // les 5 autres sections
```

- `ConnectionState` : `noDaemon` / `connecting` / `connected(StatusDTO)` /
  `daemonRestarting`. Piloté par `IpcClient` (échecs) + `DaemonManager`
  (process mort).
- **Boucle de reconnexion** : backoff court (0,2 s → 2 s). Indispensable car
  rename/force-server/reconnect provoquent un relaunch du démon (documenté à
  l'étape 1) : l'app doit vivre la coupure comme un événement normal.
- `StatusDTO`, `RequestDTO`, `ResponseDTO` : `Codable`, snake_case via
  `keyDecodingStrategy` — copie conforme du protocole v1.

## Découpage en tâches

### 1. Projet Xcode
- Créer `apps/macos/Nomad.xcodeproj` : cible app SwiftUI, macOS 14,
  `LSUIElement`, sandbox off, icône temporaire (réutiliser le disque bicolore
  du tray, en asset).
- `.gitignore` : `xcuserdata/`, `DerivedData/`.

### 2. Couche IPC (`IpcClient`)
- DTOs `Codable` du protocole v1 + tests unitaires de décodage (fixtures JSON
  copiées des tests Rust de `nomad-ipc` pour garantir la symétrie).
- `NWConnection` socket Unix, framing par lignes, file de requêtes avec
  corrélation `id`, `AsyncStream` d'événements `status`.
- Reconnexion automatique avec backoff ; expose `ConnectionState`.

### 3. Cycle de vie du démon (`DaemonManager`)
- Détection : tentative de connexion au socket (même sémantique que le test
  d'instance unique de l'étape 1).
- Spawn : `Process` avec `--headless` (+ `--ipc-socket` si surchargé),
  stdout/stderr redirigés vers le log de l'app (`os.Logger`).
- Supervision : si le process enfant meurt sans `quit` demandé → état
  `daemonRestarting`, relance avec garde anti-boucle (3 essais, puis erreur
  visible dans l'UI).
- **Attention au relaunch Rust** : `relaunch()` spawne un *nouveau* process et
  tue l'ancien → le fils de l'app disparaît alors qu'un démon tourne toujours.
  Le manager doit donc, sur mort du fils, **d'abord re-sonder le socket** avant
  de relancer quoi que ce soit.

### 4. Barre de menus (parité tray)
- `MenuBarExtra` : mêmes items d'info (rôle, nom, écran, pairs avec « (actif) »,
  « Contrôlé à distance » côté client) et mêmes actions, plus
  « Ouvrir la console… ».
- Renommer : alerte SwiftUI avec `TextField` (remplace `tinyfiledialogs`),
  validation nom non vide et différent.
- « Forcer le rôle serveur » désactivé si déjà serveur ; états dégradés :
  démon absent → item « Démarrer le service », erreurs affichées dans le menu.

### 5. Fenêtre console
- `NavigationSplitView` ; sidebar : les 6 sections + carte d'état en bas
  (pastille, rôle · nom, n clients, port) comme la maquette.
- `MachinesView` : liste des pairs du `StatusDTO` (nom, badge Serveur/Client,
  écran actif). Les colonnes riches (IP, latence, OS) arrivent à l'étape 3 —
  ne pas surdimensionner ici.
- `PlaceholderView` : icône + « Arrive à l'étape N » pour les 5 autres.
- Réglages basiques (`Settings` scene) : chemin du démon (dev), chemin du socket.

### 6. Bascule côté Rust (petite)
- README/CLAUDE.md : sur macOS, le mode recommandé devient `Nomad.app`
  (démon en `--headless`). Le tray `nomad-ui` reste tel quel — fallback
  Windows et usage CLI direct. **Aucun changement de code Rust requis** ;
  l'invariant thread principal est respecté d'office (headless ⇒ la capture
  garde le main thread du démon).

### 7. CI (léger)
- Job GitHub Actions `xcodebuild build` (sans signature) sur le runner macOS
  existant de `release.yml`, en `workflow_dispatch`/PR seulement — pas encore
  de distribution (étape 9).

## Scénarios de validation manuelle

1. **Démon absent** : lancer l'app → elle démarre le démon, la barre de menus
   affiche le rôle en ≤ 3 s.
2. **Démon préexistant** : `cargo run -- --headless` puis lancer l'app → elle
   s'attache sans spawner ; quitter l'app → le démon survit.
3. **Renommer** depuis la barre de menus → coupure IPC, reconnexion auto,
   nouveau nom affiché ; `config.toml` mis à jour.
4. **Deux machines** : brancher un client réel → il apparaît dans le menu et
   dans Machines ; le débrancher → il disparaît (via l'événement `status`).
5. **Forcer serveur** depuis un client → relaunch, rôle Serveur affiché.
6. **Crash du démon** (`kill -9`) → l'app re-sonde, relance, se reconnecte.

## Critères d'acceptation

- [ ] Parité fonctionnelle complète avec le tray actuel sur macOS.
- [ ] Fenêtre console avec sidebar 6 sections, Machines alimentée en direct.
- [ ] Les 6 scénarios manuels ci-dessus passent.
- [ ] `cargo test --workspace` intact ; le tray reste fonctionnel si on lance
      `nomad` sans l'app.
- [ ] L'app compile en une commande documentée (`xcodebuild` ou ouverture Xcode).

## Risques & points d'attention

- **`NWEndpoint.unix` + framing** : le repli BSD socket est prévu et isolé
  derrière le protocole Swift de `IpcClient` — décision à prendre dès la
  tâche 2, pas en fin d'étape.
- **Permissions macOS** : Accessibilité / Surveillance de l'entrée sont
  accordées **au démon** (le binaire `nomad`), pas à l'app. En dev, le chemin
  `target/debug/nomad` change de signature à chaque build → re-autorisation
  possible. Documenter ; le vrai flow d'onboarding est à l'étape 9.
- **Deux sources de vérité UI** (tray Rust + app) : sur macOS on n'en garde
  qu'une à l'exécution (le démon de l'app est headless). Ne pas tenter de les
  faire cohabiter dans le même process.

## Hors périmètre (étapes ultérieures)

Pages riches (layout 2D, clavier, presse-papiers, raccourcis), launchd,
bundling du démon dans l'app, signature/notarisation, IP/latence par pair
(étape 3), thème/branding final.
