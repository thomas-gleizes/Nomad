# Étape 4 — Disposition 2D des écrans

> Plan détaillé de l'étape 4 de [ROADMAP.md](ROADMAP.md). La grosse étape cœur :
> les écrans deviennent des rectangles positionnés dans un plan virtuel, les
> adjacences (4 directions) en sont **dérivées**, la disposition est persistée
> et éditable à chaud — par l'API IPC et par un canvas drag & drop dans l'app.
>
> Dépend des étapes 1–3 (livrées). Comble deux « known gaps » : la disposition
> TOML configurable et les transitions haut/bas.
>
> **Statut : implémentée.** `Layout` v2 (positions + `neighbor_at`) et son socle
> de tests géométriques, `edge.rs` porté en 2D, `placement.rs` (pur, testé),
> orchestrateur (persistance `screens`, `set_layout` validé à chaud), état
> `AppStatus.layout`, commande IPC `set_layout` + `ipcctl set-layout`, page
> Disposition SwiftUI (canvas glissable, aimantation, lecture seule côté client).
> `cargo test --workspace` et `swift build` verts.
>
> **Non vérifié ici** (nécessite un serveur vivant / la permission Accessibilité
> et une session GUI) : le drag de bout en bout et les transitions haut/bas sur
> deux machines réelles. La géométrie, le placement, l'edge 2D et la validation
> `set_layout` sont couverts par tests unitaires + loopback ; le schéma `layout`
> et le rejet de `set_layout` hors-orchestrateur sont vérifiés via `ipcctl`.
>
> **Reporté volontairement** (voir « Hors périmètre ») : barres de transition
> dessinées dans le canvas, réglages fins de bord (résistance, coins protégés,
> profils). Le bouton « Aligner automatiquement » est, lui, présent.

## Objectif

Remplacer la rangée horizontale codée en dur (`Layout::horizontal_row`, ordre
de connexion) par une **disposition 2D libre** : chaque écran a une position
`(x, y)` en pixels dans un plan virtuel partagé ; le curseur transite par tout
bord dont les rectangles se touchent, y compris verticalement et entre écrans
partiellement alignés. L'utilisateur réarrange les écrans en les glissant dans
la page Disposition de l'app.

## Le problème central : du ratio de bord à la géométrie

Aujourd'hui ([layout.rs](../../crates/nomad-core/src/layout.rs)) une transition
mappe le **ratio perpendiculaire du bord entier** de l'écran quitté vers le bord
entier de l'écran atteint (`entry_ratio(side, perp)`). Avec deux écrans de
hauteurs différentes ou décalés, ce mapping « étire » le mouvement et rend
impossible :

- deux voisins sur un même côté (ex. deux écrans empilés à droite) ;
- un écran posé au-dessus d'un coin de l'écran serveur ;
- une zone de bord *sans* transition (le curseur doit buter là où rien ne touche).

Le modèle v2 travaille en **coordonnées du plan virtuel** : sortir de l'écran A
par la droite à la hauteur `y_plan` sélectionne l'écran dont le bord gauche est
au contact **et dont l'intervalle vertical contient `y_plan`** ; le point
d'entrée est `y_plan` converti dans le repère local du voisin. Pas de ratio,
pas d'étirement.

## Décisions de conception

| Sujet | Décision | Pourquoi |
|---|---|---|
| Modèle | `Layout` v2 : `nodes: Vec<NodeInfo>` (inchangé) + `positions: HashMap<NodeId, (i32, i32)>` (origine de chaque écran dans le plan). Le rectangle d'un nœud = position + sa `Screen`. `neighbors` (la map codée en dur) **disparaît** ; l'adjacence devient une requête géométrique. | Une seule source de vérité (les positions) ; l'adjacence dérivée ne peut pas se désynchroniser. |
| Requête d'adjacence | `Layout::neighbor_at(from, side, coord) -> Option<(NodeId, (f64, f64))>` : bords au contact (tolérance ~2 px) **et** `coord` dans l'intervalle perpendiculaire partagé → renvoie le voisin + le point d'entrée en **pixels locaux du voisin**. Sinon `None` (bord du monde → clamp, comportement conservé). | Fonction pure dans `nomad-core`, testable exhaustivement ; supporte nativement N voisins par côté. |
| Wire (bincode) | `Layout` change de forme → **rupture de compatibilité** des messages `Welcome`/`LayoutUpdate` entre versions différentes. Assumé : pas de négociation de version aujourd'hui, tous les nœuds doivent tourner la même version (déjà vrai de facto). | Une couche de compat coûterait plus que le projet ne le justifie à ce stade. |
| Défaut & placement | Constructeur `Layout::row(nodes)` (remplace `horizontal_row`) : serveur à `(0,0)`, chaque suivant collé à droite, **alignés en haut**. À l'arrivée d'un client : position **persistée** si elle existe et ne chevauche rien, sinon collé à droite de l'écran le plus à droite. Au départ : la position reste persistée (la machine revient à sa place). | Comportement actuel préservé pour un setup vierge ; les habitudes de l'utilisateur survivent aux reconnexions et redémarrages. |
| Persistance | `Config.screens: Vec<{ id, x, y }>` (`#[serde(default)]`). Écrivain unique : l'orchestrateur serveur, même mécanique `reload-modify-save` que `known_peers` ([orchestrator.rs](../../crates/nomad-app/src/orchestrator.rs), `persist_known`). | Comble le gap « TOML-configurable layout » sans nouvel acteur config. |
| Édition à chaud | Commande IPC `set_layout` (champ `layout: [{node, x, y}]` dans `Request`) → `ControlCmd::SetLayout` (canal de contrôle existant, pas de relaunch). L'orchestrateur valide, applique (`ctrl.set_layout`), rediffuse `LayoutUpdate`, persiste, met à jour l'état. | Réutilise le chemin à chaud introduit pour `forget` ; protocole IPC toujours v1 (champs additifs). |
| Validation `set_layout` | Rejetée si : id inconnu, rectangles qui **se chevauchent**. Îlots (écran sans contact) acceptés avec avertissement dans la réponse — pas de transition vers eux, c'est visible dans l'UI. | Le chevauchement rend l'adjacence ambiguë ; l'îlot est un état transitoire légitime pendant un drag. |
| Exposition à l'UI | `AppStatus.layout: Vec<{ id, x, y, width, height }>` (additif, `#[serde(default)]`) — inclut le nœud local. L'app reçoit la géométrie par le flux `subscribe` existant ; pas de `get_layout`. | Un seul canal d'état, l'UI reste purement réactive. |
| `EdgeController` | Reste pur et garde son API (`local_move`, `remote_advance`, `MoveOutcome` en ratios) ; seule la résolution de voisin change (`neighbor_at` au lieu de `neighbor` + `entry_ratio`). `exit_side`, l'ancrage au bord (`edge_anchor`), `MotionTracker` : **inchangés**. | Le contrat orchestrateur↔contrôleur ne bouge pas ; la refonte est contenue dans la résolution géométrique. |
| UI — périmètre | Page Disposition : canvas à l'échelle, tuiles glissables, **aimantation** aux bords des autres tuiles + grille, envoi `set_layout` au lâcher, bouton « Aligner automatiquement » (recalcule une rangée). Les réglages fins du bord (résistance, coins protégés, profils) restent **hors périmètre** (étape ultérieure, comme prévu au ROADMAP). | Livrer le geste principal (déplacer un écran haut/bas/gauche/droite) sans se perdre dans l'inspecteur. |

## Découpage en tâches

### 1. `nomad-core` — géométrie (le socle)
[layout.rs](../../crates/nomad-core/src/layout.rs) :
- `Layout` v2 (`positions`), `rect_of(id)`, constructeur `row(nodes)`.
- `neighbor_at(from, side, coord)` : contact (tolérance 2 px) + recouvrement
  perpendiculaire ; point d'entrée en px locaux du voisin, clampé dans
  l'intervalle partagé (garde `-1` du bord de déclenchement).
- Suppression de `neighbors`/`neighbor`/`entry_ratio` (plus d'appelants après
  la tâche 2).
- **Tests** (le gros de l'étape avec la tâche 2) : rangée simple (équivalence
  avec l'ancien comportement), empilement vertical, écrans décalés
  (l'entrée conserve la coordonnée du plan), deux voisins sur un même côté,
  coin sans contact → `None`, écrans de tailles différentes, tolérance de
  contact, îlot.

### 2. `nomad-app` — `edge.rs` en 2D
- `local_move` : position de sortie convertie en coordonnée plan
  (`pos_self + (x, y)`), résolution via `neighbor_at`, `virtual_pos` = point
  d'entrée local du voisin ; `entry` (ratios) dérivé de ce point.
- `remote_advance` : idem depuis l'écran actif ; « bord du monde » inchangé.
- Porter les tests existants sur `Layout::row` + nouveaux cas 2D : sortie par
  le haut/bas, saut distant→distant vertical, franchissement dans une zone
  sans voisin (clamp), écrans décalés (pas de téléportation du point d'entrée).

### 3. `nomad-app` — orchestrateur & persistance
- Politique de placement à l'arrivée (persistée sinon append-droite) — fonction
  pure à côté de `known.rs`, testée (collisions comprises).
- `Config.screens`, persistance sur changement (join avec nouvelle position,
  `set_layout`).
- `ControlCmd::SetLayout(Vec<(NodeId, i32, i32)>)` : validation (ids,
  chevauchements), application, rediffusion, persistance, sync état. Réponse
  d'erreur propre si invalide (nécessite un retour vers l'IPC : le canal de
  contrôle gagne un `oneshot` de résultat, ou la validation est dupliquée côté
  IPC — trancher à l'implémentation, préférence pour le `oneshot`).
- `AppStatus.layout` maintenu par le serveur **et** le client (depuis les
  `LayoutUpdate` reçus).

### 4. `nomad-ipc` — commande `set_layout`
- `Request.layout: Option<Vec<LayoutEntryDTO>>`, dispatch, réponse
  ok/erreur de validation.
- `ipcctl set-layout '<json>'` + affichage du layout dans `status`.
- Tests loopback : routage, layout invalide rejeté.

### 5. App macOS — page Disposition
- `LayoutView` remplace le placeholder : canvas (échelle auto pour tenir dans
  la vue), tuiles nommées avec résolution, drag avec aimantation (grille 8 px +
  bords des voisins), barres de transition sur les segments au contact
  (dérivées côté Swift, affichage seulement), envoi `set_layout` au lâcher,
  retour visuel si le démon rejette (chevauchement).
- Bouton « Aligner automatiquement » (rangée recalculée → `set_layout`).
- États : disposition en lecture seule côté client (seul le serveur applique) —
  griser le canvas avec un bandeau « géré par le serveur » si `role == client`.

### 6. Documentation & finitions
- CLAUDE.md : modèle Layout v2, commande `set_layout`, section « Control flow »
  (la phrase « configurable TOML layout not yet implemented » saute), known gaps.
- Cocher ROADMAP ; statut dans ce plan.

## Critères d'acceptation

- [ ] `cargo test --workspace` vert ; les scénarios existants (rangée) donnent
      les mêmes transitions qu'avant la refonte.
- [ ] Tests 2D : un écran posé **au-dessus** du serveur est atteint en sortant
      par le bord haut, au bon endroit (coordonnée plan conservée) ; une zone
      de bord sans voisin bute (clamp) ; deux voisins empilés sur la droite
      sont départagés par la hauteur de sortie.
- [ ] `ipcctl set-layout` déplace un écran à chaud : `status.layout` reflète,
      `LayoutUpdate` rediffusé, `config.toml` mis à jour, et la disposition
      survit à un redémarrage du démon.
- [ ] `set_layout` avec chevauchement → réponse `ok:false` explicite.
- [ ] Dans l'app : glisser un écran au-dessus d'un autre, lâcher → la
      disposition s'applique (visible dans `ipcctl status`) ; sur deux machines
      réelles, le curseur transite ensuite par le bord haut.
- [ ] Une machine qui se reconnecte retrouve sa position persistée.

## Risques & points d'attention

- **Rupture wire** : un nœud v-ancienne ne peut plus parser `Welcome`
  (bincode). À signaler dans les notes de release ; tous les nœuds doivent être
  mis à jour ensemble.
- **Arithmétique des intervalles** (off-by-one, clamps aux extrémités du
  segment partagé, curseur pile dans un coin) : c'est le nid à bugs de l'étape —
  d'où la densité de tests demandée aux tâches 1–2 ; en cas de doute, propriété
  à préserver : *le point d'entrée est toujours dans le rectangle cible, jamais
  dans la zone de déclenchement du bord opposé*.
- **`set_layout` pendant un contrôle distant** : l'écran actif peut être
  déplacé (le contrôle y reste, seule la géométrie des prochains
  franchissements change) — comportement à couvrir par un test ; s'il
  disparaît du layout, `set_layout` du contrôleur ramène déjà en local.
- **Drag SwiftUI** : rester sur une aimantation simple (grille + bords flush) ;
  pas de moteur de contraintes. Le démon revalide de toute façon.
- **Îlots pendant le drag** : l'app n'envoie `set_layout` qu'au lâcher, jamais
  pendant le mouvement — pas de layouts transitoires diffusés aux clients.

## Hors périmètre (étapes ultérieures)

Résistance du bord / délai / coins protégés / double-toucher (réglages fins du
comportement de bord), profils de disposition nommés (Bureau/Maison),
multi-moniteurs par machine (un nœud = un écran virtuel, inchangé), édition de
la disposition depuis un client, raccourcis de bascule directe (étape 7).
