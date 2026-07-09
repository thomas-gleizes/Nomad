# Étape 1 — API de contrôle du démon (`nomad-ipc`)

> Plan détaillé de l'étape 1 de [ROADMAP.md](ROADMAP.md).
>
> **Statut : implémentée.** Crate `nomad-ipc` (protocole JSON-lines v1, socket
> Unix, garde d'instance unique), dérives serde sur `AppStatus`/`Role`/`PeerInfo`,
> câblage `main.rs` via un `ActionHandler` partagé tray/IPC, exemple `ipcctl`,
> tests loopback. Tous les critères d'acceptation ci-dessous sont vérifiés.
>
> Écart notable vs plan initial : `serve` est démarré **avant** la découverte
> mDNS bloquante (pas seulement `bind`), sinon le socket est lié mais n'accepte
> pas pendant la découverte et la sonde d'instance unique d'un second process
> échoue. `SharedStatus` est donc créé tôt avec un rôle provisoire, corrigé
> après l'élection.

## Objectif

Rendre le démon `nomad` pilotable par un processus externe via une API locale :
lire l'état (rôle, nom, pairs, écran actif), être notifié des changements, et
déclencher les actions existantes (renommer, forcer serveur, reconnecter,
quitter). C'est la fondation de l'app macOS (étape 2) — et de toute future
coquille native.

**Périmètre volontairement minimal** : l'IPC *expose* ce qui existe, il
n'ajoute aucune capacité métier. Les commandes réutilisent le mécanisme
`relaunch()` actuel de `main.rs` ; la reconfiguration à chaud viendra plus tard.

## Décisions de conception

| Sujet | Décision | Pourquoi |
|---|---|---|
| Transport | Socket Unix (`tokio::net::UnixListener`), cfg-gaté `unix` | Simple, local, permissions par droits fichier. Windows (named pipe) viendra avec sa coquille native ; en attendant le crate expose un stub no-op. |
| Chemin du socket | À côté du fichier de config : `Config::default_path().parent()/nomad.sock`, surchargeable par `--ipc-socket` | Un seul emplacement à connaître pour l'app ; suit déjà la convention du projet. |
| Format | JSON Lines (une trame JSON par ligne, UTF-8) | Trivial à implémenter côté Swift (`Codable`) et à tester au `nc -U`. Pas de bincode ici : le protocole IPC doit être lisible et indépendant du wire réseau interne. |
| Versionnage | Champ `"v": 1` dans la première trame de chaque connexion (les deux sens) | L'app et le démon évoluent séparément ; permet de refuser proprement un pair trop vieux. |
| Notifications | La tâche IPC sonde `SharedStatus::generation()` toutes les 250 ms et pousse un événement aux abonnés quand elle change | Même modèle que le tray (`nomad-ui/src/tray.rs`, POLL 500 ms) : zéro modification de `SharedStatus`. Un canal `tokio::sync::watch` pourra remplacer le sondage plus tard si besoin. |
| Actions | Nouveau type `DaemonAction` (remplace/absorbe `UiAction`), traité par un handler unique extrait de `main.rs`, partagé entre tray et IPC | Aujourd'hui la closure `on_action` appartient au tray ; l'IPC doit déclencher exactement les mêmes chemins (relaunch / save config / exit). |
| Instance unique | Au démarrage : tentative de connexion au socket existant. Réponse → une instance tourne déjà, on sort avec une erreur claire. Pas de réponse → socket orphelin, on l'unlink et on bind. | Évite deux démons qui se battent pour la capture ; donne à l'app un moyen fiable de détecter « le démon tourne ». |

### Protocole (v1)

Requêtes (client → démon), champ `id` opaque recopié dans la réponse :

```json
{"v":1,"id":1,"cmd":"status"}
{"v":1,"id":2,"cmd":"subscribe"}
{"v":1,"id":3,"cmd":"rename","name":"atlas"}
{"v":1,"id":4,"cmd":"force_server"}
{"v":1,"id":5,"cmd":"reconnect"}
{"v":1,"id":6,"cmd":"quit"}
```

Réponses et événements (démon → client) :

```json
{"v":1,"id":1,"ok":true,"status":{...}}
{"v":1,"id":3,"ok":true}
{"v":1,"id":9,"ok":false,"error":"commande inconnue"}
{"v":1,"event":"status","status":{...}}
```

Payload `status` : sérialisation directe de `AppStatus` (role, self_id,
node_name, os, screen, peers, active) en snake_case. Après `subscribe`, la
connexion reçoit immédiatement un événement `status` (état courant), puis un à
chaque changement de génération. `rename`/`force_server`/`reconnect` répondent
`ok` **avant** d'exécuter le relaunch (la connexion sera coupée par la relance :
comportement documenté, l'app doit se reconnecter).

## Découpage en tâches

### 1. Sérialisation de l'état (`nomad-core`)
- Dériver `Serialize`/`Deserialize` sur `AppStatus`, `Role`, `PeerInfo`
  ([status.rs](../../crates/nomad-core/src/status.rs)) — `NodeId`, `Os`,
  `Screen` le sont déjà. `Role` en snake_case (`"server"`/`"client"`).

### 2. Crate `nomad-ipc`
Nouveau membre du workspace, dépend de `nomad-core`, `tokio`, `serde`,
`serde_json`, `tracing`.

- `protocol.rs` : types `Request`/`Response`/`Event` (serde, `#[serde(tag = "cmd")]`
  pour les requêtes), constante `VERSION`.
- `server.rs` : `pub async fn serve(path: PathBuf, status: SharedStatus, actions: mpsc::Sender<DaemonAction>)`
  - bind avec la logique instance-unique (connect-probe → unlink si orphelin) ;
  - une tâche par connexion : lecture ligne à ligne (`BufReader::lines`),
    dispatch, écriture des réponses ;
  - registre des abonnés + tâche de sondage de `generation()` (250 ms) qui
    diffuse l'événement `status` ; abonnés morts purgés à l'écriture en échec ;
  - suppression du socket à l'arrêt propre (`Drop`/quit).
- `lib.rs` : réexports + stub non-unix (`serve` renvoie `Ok(())` et log un
  avertissement).
- `DaemonAction` vit dans `nomad-ipc` (ou `nomad-core` si le tray doit le
  partager sans dépendre d'ipc — à trancher à l'implémentation ; le plus simple :
  garder `UiAction` côté tray et convertir).

### 3. Câblage dans `nomad-app`
- [main.rs](../../crates/nomad-app/src/main.rs) :
  - extraire la closure `on_action` (l. 144-170) en une fonction/struct
    `ActionHandler` réutilisable (possède `cfg`, `config_path`, `base_args`) ;
  - créer un canal `mpsc` d'actions ; le tray et l'IPC y poussent ; une tâche
    tokio consomme et appelle `ActionHandler` (relaunch/exit fonctionnent
    depuis n'importe quel thread) ;
  - `rt.spawn(nomad_ipc::serve(...))` avant la bascule serveur/client, pour que
    l'IPC soit disponible dans les deux rôles et en headless ;
  - nouveaux flags CLI : `--ipc-socket <path>`, `--no-ipc`.
- Cas « instance déjà en cours » : message d'erreur explicite et code de sortie
  dédié (l'app macOS s'en servira pour distinguer « déjà lancé » d'un crash).

### 4. Outil de test manuel
- `crates/nomad-ipc/examples/ipcctl.rs` : mini client CLI
  (`cargo run -p nomad-ipc --example ipcctl -- status|watch|rename <nom>|...`).
  Sert aux tests manuels et de doc vivante du protocole.

### 5. Tests
- `nomad-ipc/tests/loopback.rs` (même esprit que
  [nomad-net/tests/loopback.rs](../../crates/nomad-net/tests/loopback.rs)) :
  - `status` renvoie l'état injecté dans un `SharedStatus` de test ;
  - `subscribe` : une mutation de `SharedStatus` produit un événement ;
  - commande → l'action attendue sort du canal ;
  - requête malformée → `ok:false`, la connexion survit ;
  - instance unique : second `serve` sur le même chemin échoue proprement,
    socket orphelin récupéré.
- Tests unitaires de (dé)sérialisation du protocole (round-trip serde_json).

### 6. Documentation
- CLAUDE.md : nouveau crate dans le tableau, flags CLI, protocole en une ligne.
- ROADMAP.md : cocher l'étape.

## Critères d'acceptation

- [ ] `cargo test --workspace` vert (y compris nouveaux tests) sur macOS et Linux.
- [ ] `nomad --headless` lancé, puis `ipcctl status` affiche rôle/nom/pairs.
- [ ] `ipcctl watch` reflète en direct la connexion/déconnexion d'un pair.
- [ ] `ipcctl rename foo` relance le démon avec le nouveau nom (visible dans
      `config.toml` et au `status` suivant).
- [ ] Lancer un second `nomad` → sortie immédiate avec message « déjà en cours ».
- [ ] Le tray macOS/Windows fonctionne comme avant (aucune régression).

## Risques & points d'attention

- **Relaunch vs connexions IPC** : la relance coupe le socket. Répondre `ok`
  avant de relancer, et documenter que le client doit se reconnecter (l'app de
  l'étape 2 le fait déjà par design).
- **Chemin du socket sous macOS** : `directories::ProjectDirs` peut pointer vers
  `~/Library/Application Support/...`. Vérifier la longueur du chemin
  (limite `sun_path` 104 octets) ; sinon replier sur `/tmp/nomad-<uid>.sock`.
- **Thread principal** : l'IPC vit entièrement dans tokio ; ne touche ni à la
  capture ni à l'UI → l'invariant « un seul propriétaire bloquant du thread
  principal » est préservé sans effort.

## Hors périmètre (étapes ultérieures)

Reconfiguration à chaud (sans relaunch), commandes layout/clavier/presse-papiers,
named pipe Windows, authentification du socket (droits fichier suffisent en v1).
