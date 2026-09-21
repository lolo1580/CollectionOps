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
- Intégration continue du backend : formatage, Clippy et tests sous Linux.
- Intégration continue du client Windows : restauration et compilation WinUI 3 en mode Release x64.
- Configuration typée de l’adresse d’écoute du backend.
- Endpoints distincts d’information, de vie et de disponibilité du service.
- Format JSON normalisé pour les routes introuvables.
- Génération et propagation d’un identifiant UUID `x-request-id` pour chaque requête.
- Modèle d’identité et catalogue initial de permissions effectives.
- Endpoint protégé `GET /api/v1/session`, fermé par défaut en l’absence d’identité vérifiée.
- ADR consacré à la frontière d’authentification et d’autorisation.
- Hachage local des mots de passe avec Argon2id et sels aléatoires.
- Génération de jetons de session opaques de 256 bits et empreintes SHA-256 comparées en temps constant.
- ADR définissant les règles de protection des identifiants locaux et secrets de session.

### Sécurité

- Interdiction du code Rust non sûr au niveau du workspace.
- Exclusion des fichiers de configuration locale et secrets du suivi Git.
- Aucun accès à MariaDB, stockage documentaire ou service distant dans le socle initial.
- Validation des identifiants de requête fournis par les clients et ajout des en-têtes `nosniff` et `no-referrer`.
- Séparation explicite des permissions financières et refus par défaut des accès protégés.
- Expurgation des secrets et empreintes dans leurs représentations de débogage.
