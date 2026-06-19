# no-border

Partage **souris / clavier / presse-papiers** entre plusieurs machines d'un même
réseau local — un clone minimaliste de *Mouse Without Borders*, écrit en Rust et
cross-OS (macOS, Windows, Linux/X11).

Au lancement, l'application cherche un serveur sur le LAN (mDNS) :
- si elle en trouve un → elle le rejoint comme **client (écran)** ;
- sinon → elle devient elle-même **serveur (contrôleur)**.

Le serveur est la machine où sont branchés le clavier et la souris physiques. Son
curseur traverse vers une machine voisine lorsqu'il atteint un **bord d'écran**,
exactement comme un bureau étendu.

## Architecture

Workspace Cargo en 5 crates, du plus pur au plus dépendant de l'OS :

| Crate | Rôle |
|-------|------|
| `nb-core` | Types partagés : protocole (`Message`), événements portables (`InputEvent`, `Key`, `Button`), disposition (`Layout`), codec de trame. Aucune dépendance OS — entièrement testable. |
| `nb-net` | Découverte mDNS (`mdns-sd`), élection de rôle, transport TCP (hub en étoile, tokio). |
| `nb-input` | Capture (`rdev`) et injection (`enigo`) d'entrées + table de correspondance clavier. |
| `nb-clip` | Synchronisation du presse-papiers (`arboard`). |
| `nb-app` | Binaire `no-border` : CLI, configuration, orchestration et machine d'edge-switching. |

### Flux

```
                         ┌──────────────── SERVEUR (contrôleur) ────────────────┐
  clavier/souris  ──────▶│ rdev::grab (thread principal)                        │
  physiques              │   │                                                  │
                         │   ▼  Captured                                        │
                         │ EdgeController (qui contrôle quel écran ?)           │
                         │   │                                                  │
                         │   ├─ local : OS gère nativement                      │
                         │   └─ distant : InputEvent ──▶ TCP ──▶ client actif   │
                         └──────────────────────────────────────────────────────┘
                                                          │
                                                          ▼
                         ┌──────────────── CLIENT (écran) ──────────────────────┐
                         │ TCP ──▶ enigo (injection souris/clavier)             │
                         └──────────────────────────────────────────────────────┘
```

Détail de conception : sur macOS, `rdev` (event tap) doit tourner sur le **thread
principal** avec une `CFRunLoop`. Le binaire y exécute donc la capture, tandis que
le runtime tokio (réseau + orchestration) et l'injection `enigo` vivent sur
d'autres threads, reliés par des canaux.

## Compilation

```sh
cargo build --release
cargo test --workspace   # tests unitaires (codec, keymap, edge-switching) + loopback réseau
```

Le binaire est produit dans `target/release/no-border`.

## Utilisation

Sur la machine principale (celle avec le clavier/souris), puis sur chaque machine
secondaire :

```sh
no-border                      # découverte automatique du rôle
no-border --server             # forcer le rôle serveur
no-border --name "portable"    # nom affiché
no-border --port 47800         # port serveur
RUST_LOG=debug no-border       # logs détaillés
```

L'identité du nœud (UUID stable) et les préférences sont stockées dans
`~/.config/no-border/config.toml` (Linux/macOS).

La disposition par défaut aligne les machines **horizontalement, de gauche à
droite, dans l'ordre de connexion** (le serveur étant le plus à gauche).

## Permissions et limites par plateforme

- **macOS** : autorisez l'exécutable dans *Réglages Système → Confidentialité et
  sécurité* sous **Accessibilité** *et* **Surveillance des entrées**. Sans cela,
  la capture (serveur) et l'injection (client) restent silencieusement inactives.
- **Windows** : aucune permission particulière.
- **Linux/X11** : fonctionnel ; la suppression locale des entrées (`rdev` grab via
  evdev) peut nécessiter des privilèges d'accès aux périphériques. **Wayland**
  n'est pas pris en charge.

### État d'avancement

MVP fonctionnel : découverte/élection, transport, edge-switching N machines,
souris + clavier + presse-papiers texte. Pistes connues : masquer le curseur de la
machine source pendant le contrôle distant, transfert de fichiers (drag & drop),
gestion plus fine des collisions d'élection, disposition configurable en TOML.

## Licence

MIT OR Apache-2.0.
