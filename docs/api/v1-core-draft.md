# Contrat API v1 — objets du premier lot

- Statut : contrat partiellement implémenté. Les routes espaces, inventaire, recherche paginée, renommage, archivage, corbeille, restauration, catégories, champs personnalisés, renommage des définitions, relations entre objets et transferts sont exposées ; la synchronisation reste à définir.
- Base : `/api/v1`, JSON sur HTTPS, identifiants UUID en forme canonique.
- Références : [cahier des charges](../product/cahier-des-charges-v1.md), [autorisation par espace](../architecture/ADR-0004-space-authorization.md), [migration du noyau](../../backend/migrations/202609220001_core.sql).

## Règles communes

Le fournisseur d'authentification vérifie le jeton de session avant toute opération. Le backend charge l'objet, son espace courant et l'adhésion depuis MariaDB ; un `space_id` fourni par le client ne suffit jamais à autoriser l'accès. Les permissions applicatives du `Principal` et les droits de l'espace doivent tous deux être présents. Les montants financiers et documents ne figurent pas dans les réponses de ce premier contrat.

Chaque réponse conserve l'en-tête `x-request-id`. Les erreurs utilisent le format `application/problem+json` déjà employé par le socle. `401` signifie absence de session valide ; `404` couvre un objet inexistant ou totalement inaccessible, pour ne pas révéler son existence. `403` est réservé au membre de l'espace qui ne dispose pas du droit nécessaire à l'opération. `409` indique un état concurrent ou une règle d'unicité violée. Les requêtes invalides reçoivent `422` avec un code d'erreur stable.

Le numéro d'inventaire et la révision sont des chaînes décimales positives dans le JSON. Cette représentation préserve les grands entiers pour les futurs clients Web. Le serveur attribue le numéro et la révision initiale ; aucune requête de création ou de transfert ne peut les imposer.

## Espaces et listes

`GET /api/v1/spaces` retourne `{"spaces":[...]}` avec les seuls espaces où le compte possède une adhésion et le droit explicite `collections_read` ou `acquisitions_read`. `POST /api/v1/spaces` reçoit `{"name":"Ma collection"}` et crée un espace personnel (`201`) avec droits explicites `collections_read`, `collections_write`, `acquisitions_read` et `acquisitions_write` pour son propriétaire, sans droit financier. `GET /api/v1/spaces/{space_id}/my-permissions` retourne les droits explicites du membre ; un non-membre reçoit `404`. Le partage par invitation SMTP est désormais disponible ; voir le README pour ses routes et limites. La création d'un espace partagé distinct reste à définir.

## Préparation des acquisitions

`GET` et `POST /api/v1/spaces/{space_id}/wishes` listent et créent des envies. Création : `{"title":"Casque modèle 1940","search_notes":"Chercher un exemplaire complet"}`. La liste retourne `{"wishes":[...]}`. `GET` et `POST /api/v1/spaces/{space_id}/vendors` listent et créent des vendeurs, avec `{"name":"Vendeur","website_url":"https://exemple.org"}` ; le nom est unique par espace. La liste retourne `{"vendors":[...]}`.

`GET` et `POST /api/v1/spaces/{space_id}/wishes/{wish_id}/offers` listent et créent les offres d'une envie. Création : `{"vendor_id":"<id>","title":"Annonce repérée","source_url":"https://exemple.org/annonce","notes":"À vérifier"}`. La liste retourne `{"offers":[...]}`. L'envie et le vendeur doivent appartenir au même espace ; une référence étrangère reçoit `404`. Les titres sont obligatoires (255 caractères maximum), les notes limitées à 10 000 caractères et les URL à 2 048 caractères. Seules les URL HTTP(S) avec hôte et sans identifiants sont acceptées. La lecture exige `acquisitions_read`, l'écriture `acquisitions_write`, dans l'espace concerné ; un membre dépourvu du droit reçoit `403`, un non-membre `404`. Cette tranche ne collecte ni prix ni montant et ne crée pas d'achat.

`GET /api/v1/spaces/{space_id}/items` retourne `{"items":[...],"next_cursor":null}` trié par numéro d'inventaire. L'accès requiert une session, une adhésion et `collections_read`. Un membre sans droit reçoit `403`, un non-membre reçoit `404`. La taille de page `limit` vaut 50 par défaut et doit rester entre 1 et 100. `after` est le dernier numéro d'inventaire de la page précédente ; `next_cursor` contient ce numéro comme chaîne quand une autre page existe. `q` cherche une sous-chaîne dans le nom ; `%` et `_` sont des caractères ordinaires, pas des jokers. Une recherche vide liste tous les objets. `state` filtre selon `active` (défaut), `archived`, `trashed` ou `all`. `category_id` filtre les objets classés dans la catégorie choisie ou une de ses sous-catégories actives ; `location_id` filtre l'emplacement courant et ses descendants ; `group_id` filtre les objets membres d'une série ou d'un regroupement. Un identifiant d'un autre espace reçoit `404`, un identifiant malformé `422`. Les filtres se combinent à la recherche, à l'état et au curseur. Un curseur invalide, une limite hors plage, un état inconnu ou une recherche de plus de 100 caractères reçoit `422`.

## Emplacements physiques

`GET` et `POST /api/v1/spaces/{space_id}/locations` listent et créent les emplacements de l'espace. Le corps de création est `{"name":"Étagère","parent_id":"<id facultatif>"}` ; un parent doit appartenir au même espace et les noms sont uniques parmi les enfants d'un même parent. `GET /api/v1/items/{item_id}/location` retourne `revision` et un emplacement courant ou `null`. `PUT` sur cette route reçoit `{"location_id":"<id ou null>","expected_revision":"1"}` : il déplace l'objet vers un emplacement du même espace, ou le laisse sans emplacement avec `null`. Une sélection identique ne change pas la révision et ne crée pas d'événement ; une révision obsolète reçoit `409`. Un objet en corbeille refuse le déplacement.

`GET /api/v1/items/{item_id}/location-events` retourne les mouvements du plus récent au plus ancien avec les positions de départ et d'arrivée, l'auteur et la date. Cette route ne montre que les événements de l'espace courant. Un transfert entre espaces inscrit une sortie vers `null` dans l'espace source et remet l'emplacement courant à `null` dans la destination ; les anciens mouvements restent protégés dans l'espace source. Lecture : `collections_read` ; création d'emplacement et déplacement : `collections_write`.

## Créer un objet

`POST /api/v1/spaces/{space_id}/items` exige `collections_write` au niveau applicatif et dans l'espace cible. Le corps contient seulement le nom obligatoire pour cette première tranche :

```json
{"name":"Appareil photo"}
```

Le backend retire les espaces aux extrémités du nom et refuse un nom vide ou dépassant 255 caractères après cette opération. La transaction réserve le prochain numéro de l'espace, crée l'objet et enregistre son auteur. La réponse `201` contient la représentation de l'objet (l'en-tête `Location` reste à ajouter) :

```json
{"id":"0189a4c2-7f00-7000-8000-000000000001","space_id":"0189a4c2-7f00-7000-8000-000000000002","inventory_number":"1","name":"Appareil photo","revision":"1"}
```

## Lire un objet

`GET /api/v1/items/{item_id}` exige `collections_read` dans l'espace courant de l'objet. La réponse `200` utilise la même représentation. L'identifiant de l'objet reste stable lors d'un transfert ; son `space_id` et son `inventory_number` changent.

## Renommer un objet

`PATCH /api/v1/items/{item_id}` exige `collections_write` dans l'espace courant. Le corps contient `{"name":"Nouveau nom","expected_revision":"1"}`. Le nom est nettoyé et limité à 255 caractères comme à la création. La transaction verrouille l'objet, refuse une révision obsolète avec `409`, modifie le nom et incrémente la révision. Le numéro d'inventaire et l'identifiant stable ne changent pas. L'emplacement courant se modifie par sa route dédiée.

`PUT /api/v1/items/{item_id}/details` remplace la description, la référence historique et la référence technique avec `{"description":"...","historical_reference":"...","technical_reference":"...","expected_revision":"1"}`. Ces champs sont facultatifs, nettoyés aux extrémités et vidés par `null` ou une chaîne vide. Limites : 10 000 caractères pour la description, 500 pour chaque référence. L'écriture exige `collections_write`, une révision courante et un objet hors corbeille. Les détails restent attachés à l'objet lors d'un transfert.

## Séries et regroupements

`GET` et `POST /api/v1/spaces/{space_id}/groups` listent et créent des ensembles nommés dans un espace ; `kind` vaut `series` ou `group`, et le nom est unique par type et par espace. Chaque ensemble expose une `revision` en chaîne décimale. `PATCH /api/v1/spaces/{space_id}/groups/{group_id}` reçoit `{"name":"Nouveau nom","expected_revision":"1"}` et conserve les affectations. `DELETE` sur cette route reçoit `{"expected_revision":"2"}` et ne supprime qu'un ensemble vide ; un ensemble encore utilisé reçoit `409`. Une révision dépassée reçoit aussi `409`. `GET /api/v1/items/{item_id}/groups` retourne la révision de l'objet et ses ensembles courants. `PUT` sur cette route remplace la sélection par `{"group_ids":["<id>"],"expected_revision":"1"}` ; une sélection vide retire toutes les affectations, une sélection identique ne change pas la révision, et la limite est de 50 identifiants. Les groupes d'un autre espace reçoivent `404`. La lecture exige `collections_read` et l'écriture `collections_write`. Un transfert retire les affectations de l'ancien espace avant de changer l'espace courant.

## Catégories et champs de fiche

`GET` et `POST /api/v1/spaces/{space_id}/categories` listent et créent les catégories d'un espace. La création accepte `{"name":"Casques","parent_id":"<id facultatif>"}` ; le parent doit appartenir au même espace. Les noms sont uniques parmi les enfants d'un même parent. `GET` et `POST /api/v1/spaces/{space_id}/categories/{category_id}/fields` listent et créent les champs directs (`name`, `value_type` : `text`, `number` ou `date`). Un enfant hérite des champs de tous ses ancêtres.

`GET /api/v1/items/{item_id}/categories` retourne `revision` et les catégories actuelles. `POST` sur la même route ajoute `{"category_ids":["<id>"],"expected_revision":"1"}` sans retirer les catégories existantes. `PUT` sur cette route remplace la sélection complète ; `category_ids:[]` retire toutes les catégories. Dans les deux cas, 20 catégories au maximum sont autorisées et chaque identifiant doit appartenir à l'espace courant. Le remplacement est atomique, n'incrémente la révision que si la sélection change et termine les anciennes affectations sans les supprimer. Les valeurs des champs encore accessibles via la nouvelle sélection sont recopiées si nécessaire ; les autres restent historiques mais disparaissent de la fiche courante. `GET /api/v1/items/{item_id}/fields` retourne les champs effectifs (hérités compris), dédupliqués par identifiant de champ. `PUT /api/v1/items/{item_id}/fields/{field_id}` accepte `{"value":"1944-06-06","expected_revision":"2"}`. Les valeurs sont validées selon leur type ; les dates suivent `AAAA-MM-JJ`. La lecture exige `collections_read`, l'écriture `collections_write` ; une révision obsolète reçoit `409`, un objet en corbeille refuse l'écriture.

La modification ou la suppression des définitions de catégories et de champs ne sont pas encore exposées.

`PATCH /api/v1/spaces/{space_id}/categories/{category_id}` renomme une catégorie avec `{"name":"Nouveau nom"}` et `PATCH /api/v1/spaces/{space_id}/categories/{category_id}/fields/{field_id}` renomme un champ. Le parent d'une catégorie, le type de valeur d'un champ et les identifiants stables ne changent pas, et les valeurs déjà enregistrées restent attachées au même champ. Un nom vide ou trop long reçoit `422` ; un nom déjà pris par une catégorie sœur ou un autre champ de la même catégorie reçoit `409`. L'écriture exige `collections_write` ; une catégorie ou un champ d'un autre espace reçoit `404`. La suppression d'une définition n'est toujours pas exposée, car l'historique des valeurs et des classements la référence encore.

## Relations entre objets

`GET /api/v1/items/{item_id}/relations` retourne `{"revision":"N","relations":[...]}` avec les liens sortants et entrants de l'objet dans son espace courant. Chaque entrée expose `id`, `kind` (`related`, `variant_of` ou `part_of`), `direction` (`outgoing` ou `incoming`), `related_item_id`, `related_item_name` et `created_at`.

`PUT` sur la même route remplace les liens **sortants** de l'objet par `{"relations":[{"target_id":"<id>","kind":"variant_of"}],"expected_revision":"1"}`. Un objet ne peut pas se lier à lui-même, la même paire (cible, type) ne peut pas être répétée et la limite est de 50 entrées. Chaque cible doit appartenir au même espace ; une cible inconnue ou d'un autre espace reçoit `404`. Une sélection identique ne change pas la révision ; une révision obsolète reçoit `409`, un objet en corbeille refuse l'écriture avec `422`. Un transfert vers un autre espace supprime tous les liens qui mentionnent l'objet. La lecture exige `collections_read`, l'écriture `collections_write`.

## Archiver, mettre à la corbeille, restaurer

`POST /api/v1/items/{item_id}/archive`, `POST /api/v1/items/{item_id}/trash` et `POST /api/v1/items/{item_id}/restore` exigent `collections_write` dans l'espace courant. Le corps contient la révision lue par le client :

```json
{"expected_revision":"2"}
```

Les états sont `active`, `archived` et `trashed`, et chaque transition est réversible. Les transitions acceptées sont : `active → archived`, `active → trashed`, `archived → trashed`, `archived → active` et `trashed → active`. Une transition non listée reçoit `422` avec le code `invalid_transition`. Une révision obsolète reçoit `409` et n'est jamais remplacée silencieusement. La réponse `200` contient l'objet mis à jour, dont l'état et la révision incrémentée.

L'identifiant stable, le numéro d'inventaire et l'historique des transferts sont conservés quel que soit l'état, et aucun numéro n'est réutilisé. Aucune suppression physique n'est exposée en V1. Un objet `trashed` est en lecture seule : le renommage et le transfert reçoivent `422` avec le code `invalid_state` tant qu'il n'est pas restauré. Chaque transition est auditée avec l'auteur et l'état avant/après.

`GET /api/v1/items/{item_id}/state-events` retourne `{"events":[...]}` avec l'identifiant de l'événement, l'auteur, son nom affiché actuel, les états avant/après et la date. Cette vue d'audit est réservée au propriétaire de l'espace courant et à l'administrateur système ; les autres comptes reçoivent `404`, même s'ils peuvent lire l'objet. Seuls les événements enregistrés dans l'espace courant sont retournés : un transfert ne révèle pas le journal de l'ancien espace. Les événements sont ordonnés du plus récent au plus ancien.

## Transférer un objet

`POST /api/v1/items/{item_id}/transfers` exige `collections_write` dans l'espace source et dans l'espace de destination. Le corps indique la destination et la révision lue par le client :

```json
{"destination_space_id":"0189a4c2-7f00-7000-8000-000000000003","destination_category_ids":["0189a4c2-7f00-7000-8000-000000000005"],"expected_revision":"1"}
```

Le backend vérifie d'abord les droits dans les deux espaces. La transaction verrouille ensuite l'objet, vérifie sa révision et son espace courant, réserve le prochain numéro de destination, augmente la révision, puis inscrit l'historique du transfert. Pour un objet déjà classé, `destination_category_ids` est obligatoire et doit contenir au moins une catégorie de destination valide ; le classement et les valeurs de l'ancien espace restent historiques et ne sont pas exposés dans le nouvel espace. Pour un objet non classé, la sélection peut être vide. Une destination identique à la source reçoit `422`. Une révision obsolète reçoit `409` et n'est jamais silencieusement remplacée. La réponse `200` contient l'objet mis à jour et le transfert :

```json
{
  "item": {"id":"0189a4c2-7f00-7000-8000-000000000001","space_id":"0189a4c2-7f00-7000-8000-000000000003","inventory_number":"1","name":"Appareil photo","revision":"2"},
  "transfer": {"id":"0189a4c2-7f00-7000-8000-000000000004","source_space_id":"0189a4c2-7f00-7000-8000-000000000002","source_inventory_number":"1","destination_space_id":"0189a4c2-7f00-7000-8000-000000000003","destination_inventory_number":"1"}
}
```

`GET /api/v1/items/{item_id}/transfers` retourne `{"transfers":[...]}` du plus récent au plus ancien. La lecture exige `collections_read` dans l'espace courant **et dans tous les espaces cités par l'historique** ; sinon la réponse est `404` pour ne pas révéler les anciens espaces. La réponse ne contient pas les droits, les données financières ni les jetons de session.

## Hors de cette première tranche

La liste des sessions et la connexion sont exposées depuis le 2026-09-22 (voir [ADR-0003](../architecture/ADR-0003-local-credentials-and-sessions.md)). Les invitations à droits sélectionnables, la gestion des membres, le renommage simple, l'archivage et la corbeille sont exposés depuis le 2026-09-23 ; l'édition enrichie, les documents, les montants et la synchronisation exigent encore leurs contrats spécifiques. L'idempotence des commandes sera définie avec le protocole de synchronisation avant ouverture aux modifications hors ligne.
