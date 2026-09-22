# ADR-0003 — Identifiants locaux et secrets de session

- Statut : accepté pour le socle
- Date : 2026-09-21
- Complété le 2026-09-22 par la politique de session ci-dessous.

## Contexte

L’authentification locale doit résister à la compromission de la base, permettre la révocation des appareils et rester compatible avec un fournisseur OIDC ultérieur. Les secrets ne doivent être ni récupérables depuis la base ni présents dans les journaux.

## Décision — mots de passe

- Les mots de passe sont hachés avec Argon2id version 1.3 (`v=19`).
- Le profil initial utilise 64 MiB de mémoire, 3 passes, 4 voies et une sortie de 256 bits, conformément au second profil recommandé par la RFC 9106.
- Chaque hachage reçoit un sel aléatoire propre et est stocké au format PHC, qui conserve l’algorithme et ses paramètres pour permettre les migrations.
- Une limite défensive de 1 024 octets empêche qu’une entrée démesurée monopolise les ressources. La politique fonctionnelle de longueur minimale sera définie séparément.
- Les paramètres devront être benchmarkés sur le serveur cible et réévalués périodiquement.
- Un éventuel secret supplémentaire (« pepper ») devra être conservé hors de MariaDB, idéalement dans un composant matériel ou un gestionnaire de secrets. Il n’est pas introduit sans infrastructure appropriée.

## Décision — sessions

- Le client reçoit un jeton opaque de 256 bits issu du générateur aléatoire cryptographique du système.
- Le jeton est encodé en Base64 URL sans remplissage et ne contient aucune identité ni donnée métier.
- Seule son empreinte SHA-256 est destinée à être persistée. La comparaison des empreintes est effectuée en temps constant.
- Le jeton complet n’est exposé qu’au moment nécessaire à son transport et ses représentations de débogage sont expurgées.
- Les jetons devront être transmis exclusivement via un canal TLS authentifié.
- La politique de durée est fixée ci-dessous ; les autres décisions restent ouvertes.
- La rotation après authentification ou changement de privilège, l’expiration absolue et d’inactivité, ainsi que la révocation par appareil seront appliquées par le futur dépôt de sessions.

## Décisions restant à prendre

- Durées d’expiration et politique « rester connecté ».
- Modèle de l’appareil, informations visibles par l’utilisateur et critères de confiance.
- Limitation des tentatives, temporisation et verrouillage progressif.
- Politique de longueur, liste de mots de passe compromis et récupération de compte.
- Stockage et rotation d’un éventuel pepper.
- Deuxième facteur d’authentification.

## Conséquences

- MariaDB ne devra jamais contenir de mot de passe ou de jeton de session en clair.
- Une fuite de la table des sessions ne fournit pas directement des justificatifs réutilisables.
- Le coût Argon2id protège les mots de passe mais impose une limitation stricte des tentatives pour prévenir l’épuisement des ressources.
- OIDC pourra produire le même `Principal` et les mêmes sessions internes sans contourner l’autorisation métier.

