# Bilan du projet `nomad`

Application de partage **souris / clavier / presse-papiers** sur LAN, cross-OS
(macOS, Windows, Linux/X11), écrite en Rust — clone minimaliste de *Mouse Without
Borders*.

## Ce qui est en place

Workspace Cargo en **5 crates** — build complet sans warning, **12 tests verts**.

| Crate | Contenu | Tests |
|-------|---------|-------|
| `nomad-core` | Protocole `Message`, `InputEvent/Key/Button` portables, `Layout`, codec de trame — pur, sans dépendance OS | codec round-trip + décodage en flux (3) |
| `nomad-net` | Découverte mDNS (`mdns-sd`), élection de rôle, transport TCP en étoile (tokio) | loopback serveur↔client (1) |
| `nomad-input` | Capture `rdev` + injection `enigo` + keymap rdev↔core↔enigo | round-trip clavier/boutons (3) |
| `nomad-clip` | Synchro presse-papiers `arboard` (thread unique, anti-écho) | — |
| `nomad-app` | Binaire `nomad` : CLI, config TOML persistante, orchestration + edge-switching | transitions de bord (5) |

### Mécanique demandée

Au lancement → recherche mDNS d'un serveur :
- **trouvé** → rejoint comme **client (écran)** ;
- **non trouvé** → devient **serveur (contrôleur)**.

Le serveur capture le clavier/souris physiques, et son curseur bascule vers les
machines voisines **au bord de l'écran** (modèle Synergy / Mouse Without Borders),
pour 3-4 machines disposées en rangée horizontale.

### Cross-OS

Contrainte macOS critique respectée : la capture `rdev` (event tap) tourne sur le
**thread principal** avec une `CFRunLoop`, tandis que le runtime tokio (réseau) et
l'injection `enigo` vivent sur d'autres threads, reliés par des canaux. Le code est
conçu pour macOS, Windows et Linux/X11.

### Smoke-test validé

Le binaire démarre, détecte l'écran (1800×1169), prend le rôle serveur, écoute sur
`:47800`, publie le service mDNS et atteint la boucle de capture.

## Ce qui reste (non implémenté)

Éléments de polish listés comme suites possibles :

- **Reconnexion automatique** côté client si le serveur tombe (actuellement la
  boucle s'arrête).
- **Anti-collision d'élection** si deux machines démarrent exactement en même temps
  (best-effort, non géré).
- **Masquage du curseur** de la machine source pendant le contrôle distant (léger
  flicker car le curseur réel est recentré à chaque mouvement).
- **Transfert de fichiers** drag & drop.
- **Disposition configurable** en TOML (au lieu de la rangée horizontale auto).

## Vérification end-to-end réelle

Nécessite **2 machines** (ou une VM Linux + le Mac) et, sur macOS, d'accorder les
permissions *Accessibilité* + *Surveillance des entrées*.

1. Lancer `nomad` sur chaque machine.
2. Vérifier la découverte/élection dans les logs.
3. Pousser le curseur au bord de l'écran pour basculer sur la machine voisine.
4. Taper au clavier, faire un copier/coller entre machines.

```sh
cargo build --release
cargo test --workspace

# Sur la machine principale (clavier/souris), puis sur les secondaires :
nomad                   # découverte automatique du rôle
nomad --server          # forcer le rôle serveur
RUST_LOG=debug nomad    # logs détaillés
```

## Prochaines étapes possibles

- Reconnexion client automatique.
- Masquage du curseur source.
- Initialiser un dépôt git + premier commit.
