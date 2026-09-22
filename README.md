# CollectionOps

CollectionOps est un ERP de gestion de collections personnelles et partagées. La V1 vise un client Windows complet, un backend Linux, une base MariaDB externe, un stockage documentaire hybride et un fonctionnement hors ligne contrôlé.

## État du projet

Le projet entre dans sa phase de développement. Le socle initial contient :

- une API Rust/Axum avec contrôle de santé et contrat OpenAPI ;
- une coquille de client Windows .NET 10/WinUI 3 ;
- la documentation de la décision technologique et de l’architecture ;
- des tests backend initiaux ;
- un [journal des modifications](CHANGELOG.md) maintenu à partir du premier changement.
- une intégration continue séparée pour le backend Linux et le client Windows.

Aucune infrastructure distante ni intégration S3 n’est créée à ce stade. Deux migrations MariaDB sont versionnées et appliquées au démarrage par le composant SQLx, mais uniquement lorsque `COLLECTIONOPS_DATABASE_URL` est défini.

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

Le socle d’autorisation représente séparément les permissions de collection, d’acquisition, de finance, de documents, de référentiel, de synchronisation et d’administration. Pour une ressource d'un espace, le backend doit aussi vérifier l'adhésion et les droits propres à cet espace ; `/api/v1/session` expose seulement les permissions applicatives du principal. Aucun fournisseur d’authentification réel n’est encore branché : cette route répond donc `401` tant qu’un composant vérifié n’a pas injecté le principal.

Les primitives locales utilisent Argon2id pour les mots de passe et des jetons de session opaques de 256 bits. Seules les empreintes des jetons sont destinées à être persistées. Les routes de connexion, la persistance et les politiques d’expiration restent volontairement absentes tant que leurs décisions fonctionnelles ne sont pas validées.

Les invitations disposent aussi d'un jeton opaque de 256 bits et d'une empreinte distincte. Le parcours d'envoi et d'acceptation n'est pas encore exposé par l'API.

## Démarrer le client Windows

Prérequis : Windows, Visual Studio avec les outils de développement WinUI, .NET 10 et le SDK Windows correspondant.

Ouvrir `client-windows/CollectionOps.Client/CollectionOps.Client.csproj` dans Visual Studio, sélectionner `x64` ou `ARM64`, puis lancer le projet. Le client est actuellement une coquille de navigation sans connexion réseau ni stockage local.

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

Sans `COLLECTIONOPS_DATABASE_URL`, le service démarre sans persistance et n'ouvre aucune connexion. Dès qu'elle est définie, la connexion devient obligatoire : si MariaDB est injoignable ou si une migration échoue, le backend refuse de démarrer. Les migrations sont appliquées au démarrage, ce que suppose une seule instance pour l'instant.

Le premier administrateur n'est créé que si aucun compte ne porte déjà le drapeau d'administration ; l'opération est donc idempotente et peut rester dans la configuration. Les deux variables `EMAIL` et `PASSWORD` vont de pair : n'en définir qu'une seule fait échouer le démarrage. Le mot de passe est haché avec Argon2id avant d'atteindre MariaDB et n'apparaît ni en base ni dans les journaux. Le compte est marqué comme vérifié, puisqu'il est créé par l'exploitant.

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
