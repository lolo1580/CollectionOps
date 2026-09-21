# Journal des modifications

Ce fichier suit les principes de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le projet utilisera le versionnement sémantique lorsque les premières versions seront publiées.

## [Non publié]

### Ajouté

- Monorepo initial pour le backend, le client Windows et la documentation.
- Backend Rust/Axum avec arrêt contrôlé, journalisation et configuration de l’adresse d’écoute.
- Endpoint `GET /api/v1/health` et document OpenAPI sous `GET /api/openapi.json`.
- Tests d’intégration du contrôle de santé et du contrat OpenAPI.
- Squelette du client Windows .NET 10/WinUI 3 avec navigation latérale.
- Première décision d’architecture et vue d’ensemble du système.
- Règles de contribution et de maintenance du présent changelog.

### Sécurité

- Interdiction du code Rust non sûr au niveau du workspace.
- Exclusion des fichiers de configuration locale et secrets du suivi Git.
- Aucun accès à MariaDB, stockage documentaire ou service distant dans le socle initial.

[Non publié]: https://github.com/lolo1580/CollectionOps/compare/HEAD...HEAD

