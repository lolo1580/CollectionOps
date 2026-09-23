# Contrat API v1 — objets du premier lot

- Statut : contrat partiellement implémenté. Les routes espaces, inventaire et transferts d'objets sont exposées ; pagination et synchronisation restent à définir.
- Base : `/api/v1`, JSON sur HTTPS, identifiants UUID en forme canonique.
- Références : [cahier des charges](../product/cahier-des-charges-v1.md), [autorisation par espace](../architecture/ADR-0004-space-authorization.md), [migration du noyau](../../backend/migrations/202609220001_core.sql).

## Règles communes

Le fournisseur d'authentification vérifie le jeton de session avant toute opération. Le backend charge l'objet, son espace courant et l'adhésion depuis MariaDB ; un `space_id` fourni par le client ne suffit jamais à autoriser l'accès. Les permissions applicatives du `Principal` et les droits de l'espace doivent tous deux être présents. Les montants financiers et documents ne figurent pas dans les réponses de ce premier contrat.

Chaque réponse conserve l'en-tête `x-request-id`. Les erreurs utilisent le format `application/problem+json` déjà employé par le socle. `401` signifie absence de session valide ; `404` couvre un objet inexistant ou totalement inaccessible, pour ne pas révéler son existence. `403` est réservé au membre de l'espace qui ne dispose pas du droit nécessaire à l'opération. `409` indique un état concurrent ou une règle d'unicité violée. Les requêtes invalides reçoivent `422` avec un code d'erreur stable.

Le numéro d'inventaire et la révision sont des chaînes décimales positives dans le JSON. Cette représentation préserve les grands entiers pour les futurs clients Web. Le serveur attribue le numéro et la révision initiale ; aucune requête de création ou de transfert ne peut les imposer.

## Espaces et listes

`GET /api/v1/spaces` retourne `{"spaces":[...]}` avec les seuls espaces où le compte possède une adhésion et le droit explicite `collections_read`. `POST /api/v1/spaces` reçoit `{"name":"Ma collection"}` et crée un espace personnel (`201`) avec droits explicites `collections_read` et `collections_write` pour son propriétaire. La création d'espaces partagés et les invitations ne font pas partie de cette tranche.

`GET /api/v1/spaces/{space_id}/items` retourne `{"items":[...]}` trié par numéro d'inventaire. L'accès requiert une session, une adhésion et `collections_read`. Un membre sans droit reçoit `403`, un non-membre reçoit `404`.

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

## Transférer un objet

`POST /api/v1/items/{item_id}/transfers` exige `collections_write` dans l'espace source et dans l'espace de destination. Le corps indique la destination et la révision lue par le client :

```json
{"destination_space_id":"0189a4c2-7f00-7000-8000-000000000003","expected_revision":"1"}
```

Le backend vérifie d'abord les droits dans les deux espaces. La transaction verrouille ensuite l'objet, vérifie sa révision et son espace courant, réserve le prochain numéro de destination, augmente la révision, puis inscrit l'historique du transfert. Une destination identique à la source reçoit `422`. Une révision obsolète reçoit `409` et n'est jamais silencieusement remplacée. La réponse `200` contient l'objet mis à jour et le transfert :

```json
{
  "item": {"id":"0189a4c2-7f00-7000-8000-000000000001","space_id":"0189a4c2-7f00-7000-8000-000000000003","inventory_number":"1","name":"Appareil photo","revision":"2"},
  "transfer": {"id":"0189a4c2-7f00-7000-8000-000000000004","source_space_id":"0189a4c2-7f00-7000-8000-000000000002","source_inventory_number":"1","destination_space_id":"0189a4c2-7f00-7000-8000-000000000003","destination_inventory_number":"1"}
}
```

`GET /api/v1/items/{item_id}/transfers` retourne `{"transfers":[...]}` du plus récent au plus ancien. La lecture exige `collections_read` dans l'espace courant **et dans tous les espaces cités par l'historique** ; sinon la réponse est `404` pour ne pas révéler les anciens espaces. La réponse ne contient pas les droits, les données financières ni les jetons de session.

## Hors de cette première tranche

La liste des sessions et la connexion sont exposées depuis le 2026-09-22 (voir [ADR-0003](../architecture/ADR-0003-local-credentials-and-sessions.md)). Les invitations, les listes paginées, l'édition, l'archivage, les documents, les montants et la synchronisation exigent encore leurs contrats spécifiques. L'idempotence des commandes sera définie avec le protocole de synchronisation avant ouverture aux modifications hors ligne.
