# Journal des modifications

Ce fichier suit les principes de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le projet utilisera le versionnement sémantique lorsque les premières versions seront publiées.

## [Non publié]

### Ajouté

- Routes de connexion `POST /api/v1/sessions`, de liste `GET /api/v1/sessions` et de révocation `DELETE /api/v1/sessions/{session_id}`, montées uniquement lorsque la persistance est configurée.
- Réponse de connexion renvoyant le jeton une seule fois, dans un en-tête `x-session-token` dédié plutôt qu'un cookie ou un paramètre d'URL.
- Réponse identique pour un mot de passe faux et une adresse inconnue, afin d'empêcher l'énumération des comptes.
- Contrôle de propriété à la révocation : un compte ne peut révoquer que ses propres sessions.
- Tests d'extrémité couvrant la connexion, le choix du délai d'inactivité, la révocation immédiate depuis un autre appareil et l'indépendance des appareils.
- Troisième migration MariaDB créant les sessions par appareil, avec empreinte du jeton, délai d'inactivité, dernière activité, expiration absolue et révocation.
- Politique de session : un jeton par appareil, renouvelé à chaque connexion, délai d'inactivité choisi par le client (15 min, 30 min, 1 h ou jamais) et plafond serveur de sept jours garanti par une contrainte de schéma.
- Vérification du mot de passe Argon2id à la connexion, avec rafraîchissement de l'horloge d'inactivité et refus des jetons révoqués, expirés ou inconnus.
- Révocation d'une session et liste des sessions d'un compte, sans exposer d'empreinte ni de jeton.
- Tests d'intégration couvrant le renouvellement du jeton, l'indépendance des appareils, le plafond de sept jours, la révocation immédiate et l'expiration par inactivité.
- Deuxième migration MariaDB ajoutant l'adresse e-mail, l'empreinte Argon2id du mot de passe et l'horodatage de vérification aux comptes.
- Provisionnement idempotent du premier administrateur à partir de `COLLECTIONOPS_BOOTSTRAP_ADMIN_EMAIL` et `COLLECTIONOPS_BOOTSTRAP_ADMIN_PASSWORD`, avec refus d'une configuration partielle.
- Connexion MariaDB et application des migrations au démarrage lorsque `COLLECTIONOPS_DATABASE_URL` est défini, avec refus de démarrer si la base est injoignable.
- Type `EmailAddress` normalisant l'adresse pour le domaine, sans toucher à la partie locale, et erreurs de configuration nommant la variable fautive.
- Tests d'intégration couvrant les colonnes d'identifiants, l'absence de colonne en clair et l'idempotence du provisionnement.
- Brouillon du cahier des charges fonctionnel V1 avec critères d'acceptation et décisions métier à valider.
- Modèle conceptuel du premier lot V1 pour les comptes, espaces, adhésions et objets.
- Règle validée de numérotation séquentielle par espace et renumérotation au transfert, avec brouillon de schéma MariaDB.
- Invitation par e-mail possible avant la création d'un compte, représentée dans le modèle conceptuel et le schéma provisoire.
- Politique validée d'invitation par le propriétaire, à usage unique et valable sept jours, avec révocation et réémission.
- Droits choisis lors de l'invitation, avec lecture seule présélectionnée et aucun droit financier par défaut.
- Refus d'une invitation destinée à un membre existant et contrôle commun des permissions applicatives et des droits par espace.
- Génération et validation de jetons d'invitation opaques, avec empreintes expurgées des diagnostics.
- Retrait de l'URI des traces HTTP pour éviter de journaliser d'éventuels jetons présents dans un lien.
- Première migration MariaDB pour les comptes, espaces, droits d'espace, objets et transferts ; invitations conservées dans un brouillon séparé.
- Brouillon du contrat API v1 pour créer, lire et transférer un objet avec des contrôles d'accès par espace.
- Composant SQLx de connexion et migration MariaDB, avec test d'intégration sur une base dédiée.
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

- Le jeton de session circule dans un en-tête dédié, jamais dans un cookie ni une URL, pour ne pas fuiter par `Referer` ou par l'historique.
- La connexion ne révèle pas si une adresse existe : mot de passe faux et compte inconnu produisent la même réponse.
- Un compte ne peut révoquer que les sessions qui lui appartiennent.

- Seule l'empreinte SHA-256 du jeton de session est persistée ; le jeton en clair n'existe que dans la réponse de connexion et n'est jamais journalisé.
- Le délai d'inactivité et l'expiration absolue sont vérifiés côté serveur à chaque requête ; un réglage client ne peut pas prolonger la vie d'un jeton au-delà de sept jours.
- Le schéma refuse toute session dont l'expiration absolue dépasse sept jours après sa création.

- Le mot de passe du premier administrateur est haché avant l'insertion et sa représentation de débogage est expurgée.
- Aucune colonne ne peut contenir un mot de passe en clair ; les tests le vérifient sur le schéma et sur la valeur stockée.

- Interdiction du code Rust non sûr au niveau du workspace.
- Exclusion des fichiers de configuration locale et secrets du suivi Git.
- Aucun accès à MariaDB, stockage documentaire ou service distant dans le socle initial.
- Validation des identifiants de requête fournis par les clients et ajout des en-têtes `nosniff` et `no-referrer`.
- Séparation explicite des permissions financières et refus par défaut des accès protégés.
- Expurgation des secrets et empreintes dans leurs représentations de débogage.
