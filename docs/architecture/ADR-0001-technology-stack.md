# ADR-0001 — Socle technologique de la V1

- Statut : accepté
- Date : 2026-09-21

## Contexte

CollectionOps nécessite un client Windows moderne, un backend Linux, une API commune aux futurs clients, un fonctionnement hors ligne et une persistance relationnelle centralisée.

## Décision

- Backend en Rust avec Axum et Tokio.
- Client Windows en C#/.NET 10 avec WinUI 3 et Windows App SDK.
- Contrat HTTP décrit en OpenAPI et versionné sous `/api/v1`.
- MariaDB accessible exclusivement par le backend ; SQLx est retenu pour la future couche d’accès aux données.
- Stockage hors ligne prévu avec SQLite chiffré, sans implémentation tant que le protocole de synchronisation et le modèle de menace ne sont pas validés.
- Organisation en monorepo pour versionner ensemble contrats, backend, client et documentation.

## Conséquences

- La compilation du client nécessite Windows, Visual Studio et la charge de travail WinUI.
- Le backend reste portable et compilable sous Linux.
- Le client Web futur devra consommer les mêmes contrats API et ne partager aucune connexion directe à MariaDB.
- Toute modification structurante fera l’objet d’un nouvel ADR plutôt que d’une réécriture silencieuse de cette décision.

