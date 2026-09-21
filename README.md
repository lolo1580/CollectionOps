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

Aucune base de données, infrastructure distante ou intégration S3 n’est créée à ce stade.

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

Le socle d’autorisation représente séparément les permissions de collection, d’acquisition, de finance, de documents, de référentiel, de synchronisation et d’administration. Aucun fournisseur d’authentification réel n’est encore branché : `/api/v1/session` répond donc `401` tant qu’un composant vérifié n’a pas injecté le principal.

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

GitHub Actions exécute automatiquement ces contrôles pour le backend. Un second workflow restaure et compile le client WinUI 3 sur un runner Windows x64. Aucun workflow ne déploie l’application.

## Planification

Le travail est organisé dans les [milestones GitHub](https://github.com/lolo1580/CollectionOps/milestones). Les fonctionnalités ne doivent pas être implémentées avant validation de leurs règles métier et critères d’acceptation.

## Documentation

- [Vue d’ensemble de l’architecture](docs/architecture/overview.md)
- [ADR-0001 — Socle technologique](docs/architecture/ADR-0001-technology-stack.md)
- [ADR-0002 — Frontière d’authentification et d’autorisation](docs/architecture/ADR-0002-authentication-boundary.md)
- [Journal des modifications](CHANGELOG.md)
- [Guide de contribution](CONTRIBUTING.md)
