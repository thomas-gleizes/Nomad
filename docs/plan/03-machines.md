# Étape 3 — Page Machines (état enrichi + machines connues)

> Plan détaillé de l'étape 3 de [ROADMAP.md](ROADMAP.md).
>
> **Statut : moitié démon implémentée.** État enrichi (`PeerInfo` os/addr/latence,
> `KnownPeer`, `AppStatus.known_offline`/`server_addr`), adresses plombées dans
> `nomad-net`, latence Ping/Pong 5 s dans les deux sens, machines connues
> persistées en config, commande à chaud `forget`. Module pur `known.rs` +
> tests loopback `forget`. **Moitié UI (`MachinesView`) : à faire après l'étape 2.**
>
> Non observable en test manuel sur cette machine (la capture macOS tue le
> serveur après élection, faute de permission Accessibilité) : la population
> live de `known_offline`, la latence et `forget` de bout en bout — chacun
> couvert par les tests unitaires (`known.rs`) et loopback (routage `forget`).
>
> Deux moitiés indépendantes : la moitié **démon** (Rust) ne dépend que de
> l'étape 1 (livrée) et se valide via `ipcctl` ; la moitié **UI** (Swift) exige
> le squelette de l'étape 2.

## Objectif

Donner à la page Machines tout ce que la maquette affiche : pour chaque pair
son **OS, IP, résolution et latence** ; la liste des **machines connues hors
ligne** (mémorisées entre les sessions, avec « dernière vue ») et l'action
**« Oublier »** ; côté client, l'adresse du serveur et la latence vers lui.

## Décisions de conception

| Sujet | Décision | Pourquoi |
|---|---|---|
| Protocole réseau (bincode) | **Inchangé.** Ni `Message` ni `NodeInfo`/`Layout` ne bougent. | Compatibilité entre nœuds de versions différentes ; tout l'enrichissement est local au serveur ou dérivé de données déjà transportées. |
| IP des pairs | `handle_conn` ([server.rs:101](../../crates/nomad-net/src/server.rs)) reçoit déjà l'adresse du `accept()` mais la jette (`_peer`). On l'ajoute à `ServerEvent::Joined` ; l'orchestrateur la garde dans une map serveur-locale `NodeId → SocketAddr` (surtout **pas** dans `NodeInfo`, qui part sur le réseau). | Le serveur est le seul à connaître les adresses ; zéro changement de wire. |
| Adresse du serveur (côté client) | `Endpoint::Client` transporte le `DiscoveredServer` (addr + node_id mDNS) jusqu'à `run_client`, qui le pose dans l'état. | L'info existe déjà dans `nomad_net::start` ([lib.rs:77](../../crates/nomad-net/src/lib.rs)) mais n'est pas plombée. |
| Latence | Ping/Pong applicatifs **existants**, sans champ `seq` : une tâche d'intervalle (5 s) dans chaque boucle d'orchestration ; au plus un ping en vol par nœud (map `NodeId → Instant`, RTT calculé au `Pong`). Le serveur doit en plus **répondre** au `Ping` d'un client (aujourd'hui ignoré : `other => debug!` dans `handle_server_event`). | Un seul ping en vol à 5 s d'intervalle rend le `seq` inutile → wire inchangé. Symétrique : le client mesure sa latence vers le serveur. |
| État exposé | `PeerInfo` enrichi : `os`, `screen`, `addr: Option<String>`, `latency_ms: Option<u32>`. `AppStatus` gagne `known_offline: Vec<KnownPeer>` et `server_addr: Option<String>` (client). Champs **additifs** → le protocole IPC reste v1. | `AppStatus` est déjà le payload IPC ; Swift `Codable` et `ipcctl` tolèrent les champs nouveaux. Côté client, `os`/`screen` des pairs viennent du `Layout` reçu (déjà transporté). |
| Machines connues | Nouveau type `KnownPeer { id, name, os, last_addr, last_seen_unix: u64 }` dans `nomad-core::status` (serde, timestamps en secondes epoch — pas de nouvelle dépendance). Persisté dans `config.toml` (`known_peers`, `#[serde(default)]` pour les configs existantes). L'**orchestrateur serveur** est l'unique écrivain : met à jour à chaque `Joined`/`Left` et sauvegarde. | Une seule source de vérité ; le type vit dans core car il apparaît dans le payload `status`. |
| Commande « Oublier » | Nouvelle commande IPC `forget` (+ champ `node` dans `Request`), `DaemonAction::Forget(NodeId)`. Contrairement aux autres actions, **pas de relaunch** : `ActionHandler` la pousse dans un petit canal de contrôle `tokio::mpsc` consommé par la boucle d'orchestration, qui retire le pair, persiste et met à jour l'état. | Relancer tout le démon pour oublier une machine hors ligne serait absurde. C'est la première (et minuscule) brique de reconfiguration à chaud — volontairement limitée à ça. |
| Bug à corriger au passage | `ActionHandler::Rename` sauvegarde un **clone périmé** de la config pris au démarrage : il écraserait les `known_peers` écrits ensuite par l'orchestrateur. Correctif : recharger la config depuis le disque, modifier le champ, sauvegarder. | Deux écrivains du même fichier ; le rechargement avant écriture suffit ici (un vrai « acteur config » unique est différé). |
| Historique de latence (sparkline) | Le démon expose la **valeur courante** seulement ; l'app garde un ring buffer côté Swift. | Pas d'état historique à gérer dans le démon. |
| Actions par ligne (UI) | Seul « Oublier » (machines hors ligne) est réel. Renommer/forcer serveur ne s'appliquent qu'au **nœud local** (le nom appartient à la config de chaque machine) — la maquette les montrait par ligne, on ne les promet pas. | Interface honnête : pas de boutons qui ne peuvent pas fonctionner. |

## Découpage en tâches

### Moitié démon (indépendante de l'étape 2)

1. **`nomad-core`** — enrichir le modèle d'état
   ([status.rs](../../crates/nomad-core/src/status.rs)) : champs de `PeerInfo`,
   type `KnownPeer`, `AppStatus.known_offline` + `AppStatus.server_addr`.
   Adapter les constructeurs/tests existants.

2. **`nomad-net`** — plomber les adresses
   - `ServerEvent::Joined` gagne `addr: SocketAddr` (depuis le `accept()`).
   - `Endpoint::Client` devient `{ handle, server: DiscoveredServer }` (ou
     équivalent) pour remonter addr/node_id du serveur.
   - Étendre `tests/loopback.rs` : `Joined` porte l'adresse.

3. **`nomad-app`** — orchestrateur serveur
   - Map `NodeId → SocketAddr` alimentée par `Joined`.
   - Bras `interval` (5 s) dans le `select!` : envoi de `Ping` à chaque client,
     map `NodeId → Instant` des pings en vol, RTT au `Pong` (aujourd'hui jeté),
     arrondi en ms dans `PeerInfo.latency_ms`.
   - Répondre `Pong` à un `Ping` reçu d'un client.
   - `sync_status_peers` enrichi (os/screen depuis `nodes`, addr, latence).

4. **`nomad-app`** — orchestrateur client
   - Pairs enrichis depuis les `NodeInfo` du `Layout` reçu (os, screen).
   - Ping périodique vers le serveur (même mécanique), latence posée sur le
     pair correspondant ; `server_addr` posé au démarrage.

5. **`nomad-app`** — machines connues, module pur `known.rs`
   - Fonctions pures et testées : intégration d'un `Joined`/`Left` dans la
     liste (mise à jour de `last_seen`/`last_addr`, dédoublonnage par id),
     `forget`, dérivation `known_offline = connues − connectées`, tri par
     `last_seen` décroissant. Le gros des tests unitaires de l'étape vit ici.
   - `Config.known_peers` (`#[serde(default)]`) ; l'orchestrateur serveur
     charge au démarrage, persiste à chaque changement.
   - Correctif `ActionHandler::Rename` : recharger la config avant d'écrire.

6. **`nomad-ipc`** — commande `forget`
   - `Request.node: Option<String>` (uuid), validation, `DaemonAction::Forget`.
   - Câblage `main.rs` : canal de contrôle vers l'orchestrateur (les autres
     actions gardent le chemin relaunch/exit).
   - `ipcctl forget <uuid>` + test loopback du routage.

### Moitié UI (après l'étape 2)

7. **`MachinesView` complète** : lignes riches (pastille, nom, badge rôle, OS,
   IP, résolution, latence), section « Hors ligne » (nom, dernière vue,
   « Oublier » avec confirmation), état vide. Ring buffer de latence côté app
   pour la sparkline (à partir des événements `status`).

8. **Docs** : CLAUDE.md (modèle d'état enrichi, commande `forget`, canal de
   contrôle), cocher ROADMAP.

## Critères d'acceptation

- [ ] `ipcctl status` montre pour chaque pair connecté : os, ip, résolution,
      et une latence non nulle après ~5 s.
- [ ] Côté client, `ipcctl status` montre `server_addr` et la latence vers le
      serveur.
- [ ] Débrancher un client → il passe dans `known_offline` avec `last_seen`
      cohérent ; le rebrancher → il revient dans `peers`.
- [ ] `known_offline` survit à un redémarrage du démon (persisté en TOML).
- [ ] `ipcctl forget <uuid>` retire la machine de l'état **et** de
      `config.toml`, sans relance du démon.
- [ ] `ipcctl rename x` ne perd plus les `known_peers` (bug corrigé).
- [ ] `cargo test --workspace` vert ; protocole réseau bincode inchangé
      (un client de la version précédente se connecte toujours).

## Risques & points d'attention

- **Précision de la latence** : mesurée à travers les canaux non bornés et le
  `select!`, elle inclut du jitter d'ordonnancement — valeur indicative,
  arrondie en ms ; ne pas la vendre comme une mesure réseau exacte.
- **Deux écrivains de `config.toml`** (handler au rename, orchestrateur pour
  les pairs connus) : le rechargement avant écriture réduit la fenêtre de
  course à quasi rien pour un usage humain ; l'unification en un seul acteur
  config est notée pour plus tard, pas dans cette étape.
- **`Endpoint::Client` change de forme** : petite retouche de `main.rs`
  (pattern matching) — attention à ne pas perturber l'ordre IPC-avant-découverte
  établi à l'étape 1.
- **mDNS sans `node_id`** (TXT absent) : `server_addr` reste rempli, l'id du
  serveur peut être inconnu jusqu'au premier `Layout` reçu — l'UI ne doit pas
  supposer qu'un pair « serveur » est identifiable avant ça.

## Hors périmètre (étapes ultérieures)

Wake-on-LAN (étape 8), reconnexion automatique du client (étape 8), historique
de latence côté démon, renommage d'une machine distante, appairage/PIN,
« Exclure » une machine connectée (nécessite un vrai kick protocolaire).
