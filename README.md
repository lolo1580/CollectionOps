# CollectionOps

CollectionOps est un ERP de gestion de collections personnelles et partagées. La V1 vise un client Windows complet, un backend Linux, une base MariaDB externe, un stockage documentaire hybride et un fonctionnement hors ligne contrôlé.

## État du projet

Le projet entre dans sa phase de développement. Le socle initial contient :

- une API Rust/Axum avec contrôle de santé et contrat OpenAPI ;
- un client Windows .NET 10/WinUI 3 avec navigation et gestion des sessions ;
- la documentation de la décision technologique et de l’architecture ;
- des tests backend initiaux ;
- un [journal des modifications](CHANGELOG.md) maintenu à partir du premier changement.
- une intégration continue séparée pour le backend Linux et le client Windows.

Aucune infrastructure distante ni intégration S3 n’est créée à ce stade. Les migrations MariaDB sont versionnées et appliquées au démarrage par SQLx, mais uniquement lorsque `COLLECTIONOPS_DATABASE_URL` est défini.

## Architecture retenue

| Composant | Technologie |
|---|---|
| Backend Linux | Rust, Axum, Tokio |
| Client Windows | C#, .NET 10, WinUI 3, Windows App SDK 2.4 |
| Contrats | HTTP/JSON, OpenAPI, préfixe `/api/v1` |
| Base centrale | MariaDB externe, accès futur via SQLx |
| Stockage hors ligne | SQLite chiffré, conception à finaliser |
| Documents | Stockage Linux et S3, conception à finaliser |

La décision complète est documentée dans [ADR-0001](docs/architecture/ADR-0001-technology-stack.md).

## Organisation du dépôt

```text
backend/            API et logique métier Rust
client-windows/     client natif Windows WinUI 3
docs/architecture/  décisions et vues d’architecture
Cargo.toml          workspace Rust
CHANGELOG.md        suivi des modifications
CONTRIBUTING.md      règles de contribution et vérifications
```

## Démarrer le backend

Prérequis : chaîne Rust stable avec `rustfmt` et `clippy`.

```bash
cargo run -p collectionops-backend
```

Par défaut, le service écoute sur `127.0.0.1:8080`. L’adresse peut être remplacée :

```bash
COLLECTIONOPS_BIND=0.0.0.0:8080 cargo run -p collectionops-backend
```

Points d’entrée initiaux :

- `GET http://127.0.0.1:8080/api/v1`
- `GET http://127.0.0.1:8080/api/v1/health`
- `GET http://127.0.0.1:8080/api/v1/health/live`
- `GET http://127.0.0.1:8080/api/v1/health/ready`
- `GET http://127.0.0.1:8080/api/v1/session` — route protégée, fermée par défaut
- `GET http://127.0.0.1:8080/api/openapi.json`

Chaque réponse contient un identifiant `x-request-id`. Un identifiant UUID fourni par le client est propagé ; toute autre valeur est remplacée. Les routes inconnues renvoient un document d’erreur JSON normalisé.

Le socle d’autorisation représente séparément les permissions de collection, d’acquisition, de finance, de documents, de référentiel, de synchronisation et d’administration. Les routes de collection vérifient le jeton de session puis l'adhésion et les droits explicites dans chaque espace. `/api/v1/session` reste une route de test avec principal injecté ; les routes métier utilisent directement les sessions persistées.

Les mots de passe utilisent Argon2id et les sessions des jetons opaques de 256 bits ; seule leur empreinte est conservée en base. Les routes de connexion, révocation et expiration sont implémentées.

Les invitations utilisent un jeton opaque de 256 bits ; seule son empreinte est enregistrée. Le lien est à usage unique et expire après sept jours. L'envoi par SMTP et l'acceptation sont exposés par l'API ; aucun secret ne figure dans la réponse de création ou les listes.

## Espaces, adhésions et autorisation

Créer un espace établit ensemble l'espace, sa ligne de compteur d'inventaire et l'adhésion de son propriétaire, qui reçoit `collections_read` et `collections_write`. La propriété n'accorde **aucun** droit financier, et aucune permission globale ne peut être enregistrée comme droit d'espace.

Chaque opération sur un espace doit vérifier deux choses : une permission applicative portée par l'identité, et un droit explicite dans cet espace, chargé depuis la base. Un identifiant d'espace connu ne suffit jamais. Les droits sont relus à chaque opération, donc un droit retiré cesse immédiatement d'agir, même si le client le croit encore valide.

Un membre ne peut pas modifier ses propres droits, et le propriétaire ne peut pas être retiré de son espace : le transfert de propriété sera une opération distincte et auditée.

Les espaces personnels peuvent être créés et listés par l'API et le client Windows. Seul le propriétaire invite par e-mail et choisit les droits d'espace ; `collections_read` est le seul droit présélectionné. Un nouvel envoi à la même adresse invalide le lien précédent. Le propriétaire ou l'administrateur système peut consulter les membres, changer leurs droits (finances incluses) et retirer un membre, mais ne peut ni se donner des droits à lui-même ni retirer le propriétaire. Les changements et retraits sont audités. Aucun rôle intermédiaire de gestionnaire n'est prévu pour l'instant.

## Attribution des numéros d'inventaire

Chaque espace possède une ligne de compteur (`inventory_counters`) initialisée à `1`. Le serveur attribue le numéro, jamais le client : la transaction verrouille la ligne du compteur, lit la valeur, l'incrémente et crée l'objet. Un échec annule aussi la réservation, donc un numéro n'est jamais gaspillé par une tentative refusée. L'unicité `(space_id, inventory_number)` reste une seconde ligne de défense.

Un transfert verrouille l'objet, vérifie la révision présentée par le client, puis réserve le prochain numéro de l'espace de destination et enregistre l'ancien et le nouveau numéro dans l'historique. L'identifiant de l'objet ne change jamais ; en revanche le numéro change, et les anciens numéros ne sont pas réutilisés. Une révision obsolète est refusée sans rien modifier.

La création, la recherche paginée par espace, la lecture et le transfert des objets sont reliés à l'API et au client Windows. Le transfert exige l'écriture dans les espaces source et destination et une révision courante ; l'historique exige la lecture de tous les espaces qu'il mentionne.

## Catégories et champs personnalisés

Chaque espace possède ses propres catégories, éventuellement imbriquées. Un objet peut être classé dans plusieurs catégories (20 au maximum). Les champs `text`, `number` et `date` sont définis sur une catégorie ; les sous-catégories héritent des champs de leurs ancêtres, et un champ commun à deux branches n'apparaît qu'une fois dans la fiche. Le classement peut être remplacé intégralement, y compris par une sélection vide. Les valeurs de champs encore accessibles sont conservées ; les autres restent sur les anciennes affectations historiques et ne figurent plus dans la fiche. Toutes les écritures sur l'objet exigent sa révision courante ; la corbeille reste en lecture seule.

Lorsqu'un objet classé change d'espace, il faut choisir explicitement une ou plusieurs catégories de destination. Le classement et les valeurs de l'ancien espace restent en base pour l'historique mais ne sont pas affichés dans le nouvel espace ; aucune valeur n'est copiée implicitement. Un objet non classé peut toujours être transféré sans catégorie. Le client Windows permet de créer les catégories, de définir leurs champs, de classer un objet, d'enregistrer ses valeurs et de choisir le classement à destination.

## Emplacements et filtre d'inventaire

Les emplacements physiques sont propres à chaque espace et peuvent être imbriqués (pièce → meuble → étagère). Un objet possède au plus un emplacement courant ; chaque changement est enregistré avec son auteur et ses positions de départ et d'arrivée. Une sélection « Sans emplacement » retire l'emplacement courant sans effacer l'historique. Lors d'un transfert entre espaces, l'emplacement source est libéré et sa sortie est inscrite dans l'historique de l'espace source ; cet historique n'est pas exposé dans l'espace de destination.

L'inventaire peut être filtré par catégorie ou emplacement, descendants compris, tout en conservant la recherche, le filtre d'état et la pagination. Le client Windows propose ces filtres dans l'écran Collection.

Chaque fiche possède aussi une description (10 000 caractères maximum), une référence historique et une référence technique (500 caractères chacune). Les valeurs vides sont enregistrées comme absentes. Une série ou un regroupement est nommé dans son espace ; un objet peut appartenir à plusieurs de chaque type. Le client Windows permet de les créer, renommer, classer et filtrer. La suppression d'un ensemble exige qu'il soit vide et une confirmation dans le client. Les affectations sont retirées lors d'un transfert vers un autre espace, tandis que la description et les références suivent l'objet.

## Archivage et corbeille

Un objet possède un état `active`, `archived` ou `trashed`. L'archivage et la mise à la corbeille sont réversibles, et aucune suppression physique n'est exposée en V1. Les routes `POST /api/v1/items/{item_id}/archive`, `POST /api/v1/items/{item_id}/trash` et `POST /api/v1/items/{item_id}/restore` exigent l'écriture dans l'espace courant et la révision lue par le client. Une transition non autorisée reçoit `422`, une révision obsolète `409` sans rien modifier. Chaque transition incrémente la révision et ajoute un événement d'audit avec l'auteur et l'état avant/après. Un objet en corbeille est en lecture seule jusqu'à sa restauration. L'identifiant stable, le numéro d'inventaire et l'historique des transferts survivent à toutes les transitions, et les numéros ne sont jamais réutilisés.

`GET /api/v1/items/{item_id}/state-events` présente cet audit au propriétaire de l'espace courant ou à l'administrateur système. Après un transfert, les événements d'un ancien espace ne sont pas révélés dans l'espace de destination. Le client Windows affiche cette chronologie dans la fiche d'objet aux personnes autorisées.

## Routes de session

| Méthode | Route | Rôle |
|---|---|---|
| `POST` | `/api/v1/sessions` | Se connecter et recevoir un jeton propre à l'appareil |
| `GET` | `/api/v1/sessions` | Lister les sessions du compte |
| `DELETE` | `/api/v1/sessions/{session_id}` | Révoquer un appareil |

Le corps de connexion accepte `email`, `password`, un `device_label` facultatif, un `idle_timeout` facultatif (`minutes_15`, `minutes_30`, `hour_1`, `never`) et un `stay_signed_in` facultatif. Sans `idle_timeout`, le défaut est `minutes_30`.

Un mot de passe faux et une adresse inconnue renvoient la **même** réponse `401`, afin que la route ne serve pas à découvrir quels comptes existent. Le jeton n'apparaît que dans la réponse de connexion.

Ces routes ne sont montées que si `COLLECTIONOPS_DATABASE_URL` est défini. Sans base, elles répondent `404` plutôt que d'échouer sur un pool absent.

## Invitations

| Méthode | Route | Rôle |
|---|---|---|
| `POST` | `/api/v1/spaces/{space_id}/invitations` | Propriétaire : envoyer une invitation SMTP (`{"email":"...","permissions":["collections_read"]}`) |
| `GET` | `/api/v1/spaces/{space_id}/invitations` | Propriétaire : voir les invitations sans jeton |
| `DELETE` | `/api/v1/spaces/{space_id}/invitations/{invitation_id}` | Propriétaire : révoquer un lien en attente |
| `POST` | `/api/v1/invitations/{invitation_id}/accept` | Accepter avec `token` ; fournir `display_name` et `password` uniquement pour créer un compte |
| `GET` | `/api/v1/admin/spaces` | Administrateur système : lister les espaces à administrer, sans accès implicite à leur inventaire |
| `GET` | `/api/v1/spaces/{space_id}/members` | Propriétaire ou administrateur : lister les membres et leurs droits |
| `PUT` | `/api/v1/spaces/{space_id}/members/{account_id}/permissions` | Propriétaire ou administrateur : remplacer les droits explicites d'un autre membre |
| `DELETE` | `/api/v1/spaces/{space_id}/members/{account_id}` | Propriétaire ou administrateur : retirer un membre autre que le propriétaire |

Le destinataire avec un compte existant doit se connecter avec l'adresse e-mail invitée, déjà vérifiée. Un destinataire sans compte crée son compte depuis le lien reçu ; la possession du lien prouve alors l'adresse. L'acceptation, la création éventuelle du compte, l'adhésion et les droits sont atomiques. Une invitation expirée, révoquée, déjà acceptée ou adressée à un membre existant ne modifie pas les droits. Les créations, révocations et acceptations sont auditées.

Le lien `collectionops://invite/...?...` se colle dans **Compte → Accepter une invitation** du client Windows. Il n'est pas encore associé automatiquement au protocole Windows ; le collage est nécessaire. Le lien contient l'adresse HTTPS du serveur configurée par l'exploitant, et le client refuse de transmettre le jeton si son adresse de serveur ne correspond pas. Le jeton n'est jamais placé dans une URL HTTP.

La fiche d'objet prend en charge le renommage par `PATCH /api/v1/items/{item_id}` avec `name` et `expected_revision`, la description et les références par `PUT /api/v1/items/{item_id}/details`, ainsi que les catégories, leurs champs personnalisés, les séries/regroupements et l'emplacement courant. Une révision dépassée renvoie `409` et n'écrase pas la modification plus récente. Un [brouillon de synchronisation hors ligne](docs/architecture/offline-sync-v1-draft.md) fixe les invariants et les questions à trancher ; le mode hors ligne n'est pas activé.

## Démarrer le client Windows

Prérequis : Windows, Visual Studio avec les outils de développement WinUI, .NET 10 et le SDK Windows correspondant.

Ouvrir `client-windows/CollectionOps.Client/CollectionOps.Client.csproj` dans Visual Studio, sélectionner `x64` ou `ARM64`, puis lancer le projet. Dans **Paramètres**, saisir l'adresse du serveur et choisir le thème. Dans **Compte**, se connecter ou accepter une invitation. Dans **Collection**, créer ou choisir un espace, envoyer des invitations si l'on en est propriétaire, gérer les membres si l'on est propriétaire ou administrateur, rechercher et parcourir l'inventaire page par page, filtrer par état (actifs, archivés, corbeille, tous), ajouter des objets, puis en sélectionner un pour voir sa fiche, le renommer, l'archiver, le mettre à la corbeille ou le restaurer, le transférer et voir son historique. Un administrateur qui n'est pas membre peut administrer les droits sans consulter l'inventaire. Le backend nécessite `COLLECTIONOPS_DATABASE_URL`. Le jeton, l'adresse du serveur et le thème restent uniquement en mémoire ; la connexion et ces réglages doivent être refaits après redémarrage. HTTPS est requis à distance, tandis que HTTP est autorisé pour un serveur local. Le mode hors ligne n'est pas encore disponible.

Depuis PowerShell, dans la racine du dépôt :

```powershell
dotnet run --project .\client-windows\CollectionOps.Client\CollectionOps.Client.csproj -p:Platform=x64
```

## Qualité

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Les consignes détaillées figurent dans [CONTRIBUTING.md](CONTRIBUTING.md).

Le test de migration et de provisionnement peut être exécuté sur une base MariaDB de test dédiée avec `COLLECTIONOPS_TEST_DATABASE_URL=mysql://... cargo test -p collectionops-backend --test database`. Sans cette variable, ces tests sont ignorés.

## Configuration

| Variable | Rôle | Défaut |
|---|---|---|
| `COLLECTIONOPS_BIND` | Adresse d'écoute HTTP | `127.0.0.1:8080` |
| `COLLECTIONOPS_DATABASE_URL` | URL MariaDB ; active la persistance | aucune |
| `COLLECTIONOPS_BOOTSTRAP_ADMIN_EMAIL` | Adresse du premier administrateur | aucune |
| `COLLECTIONOPS_BOOTSTRAP_ADMIN_PASSWORD` | Mot de passe du premier administrateur | aucune |
| `COLLECTIONOPS_BOOTSTRAP_ADMIN_NAME` | Nom affiché du premier administrateur | adresse e-mail |
| `COLLECTIONOPS_SMTP_HOST` | Nom DNS du relais SMTP STARTTLS | aucune ; invitations désactivées |
| `COLLECTIONOPS_SMTP_PORT` | Port STARTTLS du relais | `587` |
| `COLLECTIONOPS_SMTP_USERNAME` | Identifiant SMTP | aucune |
| `COLLECTIONOPS_SMTP_PASSWORD` | Secret SMTP | aucune |
| `COLLECTIONOPS_SMTP_FROM` | Adresse expéditrice SMTP | aucune |
| `COLLECTIONOPS_PUBLIC_URL` | Origine HTTPS du backend vue par le client Windows ; liée au lien d'invitation | aucune |

Sans `COLLECTIONOPS_DATABASE_URL`, le service démarre sans persistance et n'ouvre aucune connexion. Dès qu'elle est définie, la connexion devient obligatoire : si MariaDB est injoignable ou si une migration échoue, le backend refuse de démarrer. Les migrations sont appliquées au démarrage, ce que suppose une seule instance pour l'instant.

Le premier administrateur n'est créé que si aucun compte ne porte déjà le drapeau d'administration ; l'opération est donc idempotente et peut rester dans la configuration. Les deux variables `EMAIL` et `PASSWORD` vont de pair : n'en définir qu'une seule fait échouer le démarrage. Le mot de passe est haché avec Argon2id avant d'atteindre MariaDB et n'apparaît ni en base ni dans les journaux. Le compte est marqué comme vérifié, puisqu'il est créé par l'exploitant.

Les quatre variables SMTP obligatoires et `COLLECTIONOPS_PUBLIC_URL` doivent être définies ensemble, sinon le démarrage échoue. Le transport impose STARTTLS et un délai de 15 secondes. Sans configuration SMTP, le serveur reste utilisable mais la création d'invitations est indisponible. La réponse `201` signifie que le relais a accepté le message, pas que la boîte destinataire l'a livré. Aucun mot de passe SMTP ne doit être ajouté au dépôt.

Pour Infomaniak, utiliser `COLLECTIONOPS_SMTP_HOST=mail.infomaniak.com`, `COLLECTIONOPS_SMTP_PORT=587` et l'adresse e-mail complète de la boîte comme identifiant SMTP, conformément à la [documentation Infomaniak](https://www.infomaniak.com/fr/support/faq/468/comprendre-les-ports-et-protocoles-de-messagerie). Renseigner le mot de passe uniquement dans l'environnement du serveur, jamais dans Git. Il manque encore l'adresse expéditrice et un domaine HTTPS CollectionOps validés pour réaliser un essai d'invitation de bout en bout ; ne pas définir `COLLECTIONOPS_PUBLIC_URL` avec une adresse fictive.

## Durées de session

| Réglage client | Verrouillage après inactivité | Durée de vie du jeton |
|---|---|---|
| 15 minutes | 15 minutes | 12 heures |
| 30 minutes | 30 minutes | 12 heures |
| 1 heure | 1 heure | 12 heures |
| Jamais | aucun | 7 jours |

Le réglage client ne choisit que le délai d'inactivité ; la durée de vie du jeton reste décidée par le serveur et plafonnée à sept jours.

GitHub Actions exécute automatiquement ces contrôles pour le backend. Un second workflow restaure et compile le client WinUI 3 sur un runner Windows x64. Aucun workflow ne déploie l’application.

## Planification

Le travail est organisé dans les [milestones GitHub](https://github.com/lolo1580/CollectionOps/milestones). Les fonctionnalités ne doivent pas être implémentées avant validation de leurs règles métier et critères d’acceptation.

Un [brouillon du cahier des charges fonctionnel V1](docs/product/cahier-des-charges-v1.md) rassemble les exigences proposées, leurs critères d'acceptation et les décisions métier à valider pour le premier jalon.

## Documentation

- [Vue d’ensemble de l’architecture](docs/architecture/overview.md)
- [ADR-0001 — Socle technologique](docs/architecture/ADR-0001-technology-stack.md)
- [ADR-0002 — Frontière d’authentification et d’autorisation](docs/architecture/ADR-0002-authentication-boundary.md)
- [ADR-0003 — Identifiants locaux et secrets de session](docs/architecture/ADR-0003-local-credentials-and-sessions.md)
- [ADR-0004 — Autorisation par espace](docs/architecture/ADR-0004-space-authorization.md)
- [Modèle conceptuel du premier lot V1 — brouillon](docs/architecture/data-model-v1-draft.md)
- [Première migration MariaDB du noyau](backend/migrations/202609220001_core.sql)
- [Brouillon SQL des invitations](docs/schema/invitations-draft.sql)
- [Contrat API v1 des premières opérations sur les objets — brouillon](docs/api/v1-core-draft.md)
- [Journal des modifications](CHANGELOG.md)
- [Guide de contribution](CONTRIBUTING.md)
