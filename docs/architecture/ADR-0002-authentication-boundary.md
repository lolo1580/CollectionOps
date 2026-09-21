# ADR-0002 — Frontière d’authentification et d’autorisation

- Statut : accepté pour le socle
- Date : 2026-09-21

## Contexte

CollectionOps doit gérer des utilisateurs multiples, des espaces privés et partagés, des permissions personnalisables, des permissions financières indépendantes, des sessions persistantes et la révocation des appareils. Une intégration OIDC reste possible ultérieurement, mais ne doit pas imposer son modèle au domaine.

## Décision

- Le backend refuse par défaut toute route protégée lorsqu’aucune identité vérifiée n’est présente.
- Une identité vérifiée devient un `Principal` contenant un sujet stable, des rôles informatifs et la liste explicite de ses permissions effectives.
- Les contrôles métier consultent exclusivement les permissions effectives ; ils ne déduisent jamais implicitement un droit à partir du nom d’un rôle.
- Les permissions financières restent distinctes des permissions de lecture et modification des collections.
- Le contrat `/api/v1/session` expose au client l’identité et les permissions effectives de la session courante.
- Le futur fournisseur d’authentification sera responsable de vérifier les justificatifs, charger la session, appliquer les révocations et injecter le `Principal`.

## Hors périmètre de cette décision

- Format définitif des identifiants de session.
- Stockage des mots de passe et paramètres de dérivation de clé.
- Deuxième facteur d’authentification.
- Modèle des appareils et règles d’expiration.
- Adaptateur OIDC.
- Catalogue définitif des rôles prédéfinis.

Ces éléments doivent être précisés par une décision ultérieure avant leur implémentation.

## Conséquences

- En l’absence de fournisseur d’authentification, les routes protégées répondent `401` ; aucune identité de développement implicite n’est créée.
- Les rôles peuvent évoluer sans modifier les contrôles d’autorisation internes, puisque leur expansion en permissions se produit à la frontière de sécurité.
- Toute nouvelle permission doit être documentée, testée et reliée à un besoin fonctionnel validé.

