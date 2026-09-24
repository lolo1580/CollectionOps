# Décisions métier ouvertes V1 — propositions à valider

- Statut : proposition ; aucune règle n'est appliquée avant validation explicite.
- Références : [cahier des charges V1](cahier-des-charges-v1.md), [synchronisation hors ligne](../architecture/offline-sync-v1-draft.md), [autorisation par espace](../architecture/ADR-0004-space-authorization.md).
- Portée : D06 (visibilité prix/documents), D07 (règles financières), D09 (conflits hors ligne), D10 (formats d'import/export et limites documentaires).

Ce document transforme les décisions ouvertes en propositions concrètes, avec des critères d'acceptation vérifiables. Chaque section indique la règle proposée, ses conséquences techniques et ce qui reste à trancher.

## D06 — Visibilité des prix et des documents lorsque l'objet est partagé

**Règle proposée : séparation stricte par permission, sans droit implicite.**

- Lire la fiche « collection » (nom, description, références, catégories, champs personnalisés, emplacements, séries, relations, état) exige `collections_read` et ne montre **aucun** montant ni document.
- Lire un montant (prix d'acquisition, frais, estimation, valorisation, budget, résultat de vente) exige `finance_read` dans l'espace concerné. Modifier un montant exige `finance_write`.
- Lire ou ajouter un document (photographie, fichier) exige `documents_read` ou `documents_write`. Aucun document n'est copié par `collections_read`.
- Un membre qui possède `collections_read` sans droit financier reçoit un objet **sans champ financier** et un indicateur explicite du type `financial_data_hidden`, afin qu'une valeur absente ne soit jamais confondue avec zéro.
- Le transfert d'un objet vers un autre espace **ne copie** ni montants ni documents : comme pour les catégories, chaque espace garde ses propres données de domaine, et l'ancien historique reste protégé dans l'espace source.
- Les listes, l'historique de transfert, l'audit et les exports ne contiennent jamais un montant que l'appelant ne peut pas lire.

**Critères d'acceptation**

1. Un membre `collections_read` seul ne voit aucun montant ni document, même en appelant une route par identifiant connu.
2. Retirer `finance_read` bloque immédiatement toutes les routes financières (`403`), sans redémarrage.
3. Aucun total, aucune liste et aucun export ne révèle un montant inaccessible.
4. Un objet transféré ne conserve pas les montants/documents de l'espace source.

**À trancher :** les documents suivent-ils l'objet lors d'un transfert (copie explicite) ou restent-ils dans l'espace source ? Proposition : ils restent, avec un parcours de copie/partage explicite.

## D07 — Règles financières : devise, taux, arrondi, valeur sans estimation

**Règle proposée.**

- **Devise.** Chaque montant enregistre sa **devise d'origine** (code ISO 4217) et un montant exact. Une devise principale est configurable par espace. Changer de devise principale ne réécrit jamais les montants stockés.
- **Taux historique.** Un taux de change est une entité datée `(paire, taux, date, source)`. Toute conversion utilise le taux applicable **à la date de l'opération** ; le taux courant ne s'applique jamais à un calcul rétroactif. Un taux manquant est signalé, jamais remplacé par 1.
- **Arrondi.** Le montant exact est conservé ; l'arrondi n'intervient qu'à l'affichage et à l'agrégation, selon une règle documentée (proposition : demi-haut, 2 décimales pour le franc suisse). Un total indique la devise et la règle d'arrondi appliquées.
- **Estimations.** Chaque estimation est datée et conserve sa source ; une nouvelle estimation n'efface pas les précédentes. La valeur courante d'un objet est l'estimation la plus récente à la date de calcul.
- **Valeur sans estimation.** Un objet sans estimation ne vaut pas silencieusement `0` : il est compté séparément. Un total affiche « valeur des objets estimés » et le nombre d'objets non estimés. La valeur `0` explicite reste possible.
- **Budgets.** Mensuels, annuels, par catégorie ou par thème, calculés uniquement à partir de lignes, taux et dates enregistrés : un total doit être reproductible à l'identique.

**Critères d'acceptation**

1. Un total est reproductible à partir des lignes, taux et dates utilisés ; deux calculs à la même date donnent le même résultat.
2. Ajouter une estimation ne supprime aucune estimation antérieure.
3. Un objet non estimé n'est pas présenté comme valant zéro dans un total ; le compteur correspondant est visible.
4. Un taux manquant produit une erreur explicite, pas une conversion silencieuse.
5. Changer la devise principale ne modifie aucun montant enregistré.

**À trancher :** précision des montants stockés (proposition `DECIMAL(19,4)`) et liste des devises supportées en V1 (proposition : toute devise ISO 4217, avec une saisie contrôlée).

## D09 — Conflits hors ligne : fusion automatique ou choix humain

**Règle proposée.**

- MariaDB reste la source de vérité ; une copie locale ne prouve jamais un droit. Chaque commande hors ligne porte un identifiant stable, l'identifiant de l'objet, la révision serveur de départ et les champs modifiés.
- Le rejeu est **idempotent** : rejouer une commande après une coupure ne crée pas de doublon et n'incrémente pas la révision deux fois.
- **Fusion automatique** uniquement si l'ensemble des champs modifiés localement est **disjoint** de l'ensemble modifié côté serveur depuis la révision de départ, et si les champs sont commutatifs (par exemple deux champs personnalisés différents). Les deux modifications sont alors appliquées.
- **Conflit présenté à l'utilisateur** dans tous les autres cas : conserver la version locale, conserver la version serveur, ou ressaisir. Le serveur n'est jamais écrasé silencieusement.
- **Champs non fusionnables en V1** : nom, état (`active`/`archived`/`trashed`), classement, emplacement, séries/regroupements, relations. Le **transfert entre espaces n'est jamais mis en file** en V1.
- Une suppression (mise à la corbeille) hors ligne est une modification d'état ; une restauration concurrente devient un conflit.
- À la reconnexion, un droit retiré interdit immédiatement l'envoi et l'affichage ; la politique de purge de la copie locale reste à décider (voir brouillon hors ligne).

**Critères d'acceptation**

1. Rejouer une commande après une coupure ne crée pas de second objet et n'incrémente pas la révision deux fois.
2. Deux modifications de champs disjoints sont fusionnées sans intervention.
3. Toute autre divergence affiche local et serveur et exige un choix humain ; le serveur n'est jamais écrasé sans décision.
4. Un droit retiré bloque l'envoi dès la reconnexion.

**À trancher :** la politique de purge/traitement de la copie locale après révocation (purge immédiate, conservation temporaire, export chiffré) et la durée de conservation des accusés côté serveur.

## D10 — Formats d'import/export et limites documentaires

**Règle proposée.**

- **Export.** CSV pour l'inventaire (colonnes stables, UTF-8, séparateur configurable) et JSON pour un export complet (objets, catégories, champs, relations). Un export ne contient que ce que l'appelant peut lire (D06).
- **Import.** CSV ou JSON, avec **aperçu** obligatoire avant écriture et **compte rendu** ligne à ligne : lignes acceptées, lignes rejetées et cause. Un identifiant externe optionnel rend l'import idempotent : réexécuter le même fichier ne crée pas de doublons silencieux.
- **Documents.** Types acceptés : JPEG, PNG, WebP, PDF. Taille maximale par fichier : 50 Mo. Nombre maximal de documents actifs par objet : 50. Chaque document porte des métadonnées indépendantes du stockage (empreinte SHA-256, type MIME, taille, auteur, date) ; le stockage est local Linux ou S3 selon la configuration.
- **Déduplication.** Une copie dédupliquée est stockée une fois par empreinte, mais **les permissions ne se mélangent jamais** : chaque référence garde les droits de son objet. Supprimer une référence ne supprime le blob que s'il n'est plus référencé.

**Critères d'acceptation**

1. Un export puis un import dans un espace vide restitue objets, catégories, champs et relations.
2. Chaque ligne rejetée d'un import indique une cause ; réexécuter le même import ne crée pas de doublon.
3. Un document dépassant la taille, le type ou le quota est refusé avec un message explicite.
4. Un document dédupliqué n'accorde jamais un accès à un objet dont l'appelant n'a pas les droits.

**À trancher :** taille maximale des fichiers (proposition 50 Mo), quota par objet (proposition 50), et formats exacts des colonnes CSV.

## Synthèse à valider

| Décision | Proposition | Débloque |
|---|---|---|
| D06 | Droits de lecture séparés par domaine, aucun droit implicite, transfert sans copie des données de domaine | Finances, documents |
| D07 | Devise d'origine + taux daté, arrondi à l'affichage, non estimé ≠ zéro | Finances, budgets, rapports |
| D09 | Fusion auto si champs disjoints, sinon choix humain ; transfert jamais hors ligne | Protocole de synchronisation |
| D10 | Export CSV/JSON, import avec aperçu et rapport, JPEG/PNG/WebP/PDF ≤ 50 Mo | Documents, imports/exports |

Une fois ces propositions validées ou corrigées, les milestones 13 (finances), 9 (médias et continuité) et 04 (API et synchronisation hors ligne) peuvent être détaillés puis implémentés.
