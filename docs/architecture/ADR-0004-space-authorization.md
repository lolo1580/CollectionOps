# ADR-0004 — Autorisation par espace

- Statut : accepté pour le premier lot
- Date : 2026-09-22
- Clarifie la portée des permissions effectives décrites dans [ADR-0002](ADR-0002-authentication-boundary.md).

## Contexte

Un même compte peut appartenir à plusieurs espaces avec des droits distincts. Le `Principal` existant porte des permissions applicatives sans identifiant d'espace. Une vérification de ces seules permissions permettrait à un membre d'un espace de lire un autre espace s'il connaît son identifiant.

## Décision

- Les permissions du `Principal` constituent un plafond applicatif ; chaque opération sur un espace exige aussi une adhésion et un droit explicite dans cet espace.
- Les permissions de collection, acquisition, finance et documents sont vérifiées dans le contexte de l'espace. Les permissions de référentiel, synchronisation, utilisateurs, audit et administration restent hors des droits d'adhésion à un espace ; leurs propres règles seront définies avec les parcours correspondants.
- L'adhésion et l'espace visé sont chargés par le backend depuis un dépôt de confiance. Une valeur fournie par le client ne constitue jamais une preuve d'adhésion. Pour une opération sur un objet, l'espace est celui de l'objet chargé depuis le dépôt.
- Une absence d'adhésion, une adhésion à un autre compte ou espace, une permission applicative absente ou un droit d'espace absent provoquent un refus.
- La création, révocation et réémission d'invitations exigent que l'identité vérifiée soit le propriétaire courant de l'espace visé. La propriété n'accorde pas implicitement les permissions financières.
- La consultation des membres, la modification de leurs droits et leur retrait sont réservés au propriétaire courant ou à un administrateur système. Aucun rôle de gestionnaire intermédiaire n'est défini. Ces acteurs peuvent attribuer explicitement les droits financiers, sans les obtenir automatiquement par la propriété ou l'administration.
- Un administrateur système peut voir les espaces et gérer leurs membres sans être membre lui-même ; cela ne lui donne pas accès à leur inventaire. Chaque modification de droits ou retrait est audité avec l'auteur, la cible et les droits avant/après.
- L'acceptation d'une invitation par un membre déjà présent est refusée ; ses droits ne changent pas.

## Conséquences

- Le service doit relire les droits d'espace lors d'une opération protégée afin que leur retrait prenne effet côté serveur. Le cache du client n'est pas une source d'autorisation.
- Le contrat `/api/v1/session` continue de présenter les permissions applicatives, mais ne prétend pas fournir la liste complète des droits par espace.
- Les futures routes d'espace et d'objet doivent utiliser le contrôle commun `require_space_permission` après chargement de l'adhésion. Les routes d'invitation utilisent `require_space_owner` avec le propriétaire courant ; les routes de gestion des membres utilisent le contrôle propriétaire ou administrateur, également vérifié dans la transaction MariaDB.
- La matrice d'autorisations des fonctionnalités encore non spécifiées reste à compléter avant leur implémentation.
