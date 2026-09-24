# Journal des modifications

Ce fichier suit les principes de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le projet utilisera le versionnement sémantique lorsque les premières versions seront publiées.

## [Non publié]

### Corrigé

- La recherche paginée d'inventaire liait le `group_id` à la place du `space_id` dans l'ordre des paramètres SQL, ce qui renvoyait une liste vide pour tout appel à `GET /spaces/{space_id}/items` ; l'ordre des `bind` suit maintenant les `?` de la requête.
- La connexion renvoie l'identifiant et le nom du compte authentifié dans `principal`, au lieu d'un identifiant aléatoire et d'un nom vide.
- Le client Windows efface son état de connexion après un refus `401` sur les sessions et affiche une erreur contrôlée pour une réponse JSON invalide ou un état de santé inattendu.
- Quatre vérifications automatisées du client de session couvrent l'adresse serveur, l'état de santé, l'expiration de session et une réponse JSON invalide ; elles s'exécutent aussi dans la CI Windows.

### Ajouté

- Relations typées et dirigées entre objets d'un même espace (`related`, `variant_of`, `part_of`) : routes `GET` et `PUT /api/v1/items/{item_id}/relations`, remplacement d'ensemble limité à 50, contrôle de révision, refus d'un lien vers soi-même ou vers un autre espace, et suppression de tous les liens qui mentionnent un objet transféré.
- Migration `item_relations` et tests HTTP/MariaDB de l'isolation entre espaces, des révisions, de la lecture entrante/sortante, de la corbeille et du nettoyage au transfert.
- Renommage des catégories et des champs personnalisés par `PATCH`, sans changer le parent, le type de valeur ni les identifiants stables ; un nom déjà pris reçoit `409`, une définition d'un autre espace `404`, et les valeurs enregistrées restent attachées au même champ.
- En-tête `Location` sur la création d'objet, pointant vers `/api/v1/items/{item_id}`.
- Client Windows : option temporaire pour joindre directement une adresse IPv4 privée en HTTP pendant le développement, désactivée par défaut et limitée aux plages privées ; changement de serveur ou de mode invalidant la session locale.
- Envies, vendeurs et offres repérées par espace, sans montants, avec droits d'acquisition explicites, API et écran Windows ; migration des droits des propriétaires existants et tests d'isolation.
- Renommage des séries et regroupements avec contrôle de révision ; suppression des seuls ensembles vides, confirmée dans le client Windows.
- Description, références historique et technique des objets, avec validation et contrôle de révision dans l'API et le client Windows.
- Séries et regroupements par espace, classement multiple des objets et filtre d'inventaire correspondant ; les affectations sont retirées lors d'un transfert.
- Test HTTP/MariaDB du parcours fiche, des droits, des révisions, du filtrage et de l'isolation des regroupements.
- Emplacements physiques hiérarchiques par espace, position courante unique par objet et journal des déplacements, reliés à l'API et au client Windows.
- Sortie d'emplacement enregistrée lors d'un transfert entre espaces, sans révéler l'historique source dans la destination.
- Filtres d'inventaire par catégorie ou emplacement et leurs descendants, compatibles avec la recherche, les états et la pagination ; tests MariaDB/HTTP et contrat client.
- Remplacement ou retrait du classement d'un objet dans l'API et le client Windows, avec conservation des champs encore applicables et des anciennes valeurs en historique.
- Test MariaDB/HTTP des droits, révisions, catégories d'un autre espace, suppression complète du classement et préservation des valeurs héritées ; vérification du contrat client Windows.
- Catégories imbriquées par espace, classement multiple des objets et champs personnalisés texte, nombre ou date hérités des catégories parentes, avec API, migration MariaDB et interface Windows.
- Transfert d'un objet classé avec choix explicite des catégories de destination ; l'ancien classement et ses valeurs restent historiques et ne sont pas copiés vers le nouvel espace.
- Tests HTTP/MariaDB du classement, de l'héritage, des droits et du transfert, ainsi que trois vérifications du contrat client Windows.
- Consultation de l'audit des changements d'état depuis l'API et la fiche Windows, réservée au propriétaire ou à l'administrateur et limitée aux événements de l'espace courant.
- États d'objet `active`, `archived` et `trashed` : archivage, corbeille et restauration réversibles, sans suppression physique en V1, avec conservation de l'identifiant, du numéro d'inventaire et de l'historique.
- Routes `POST /api/v1/items/{item_id}/archive`, `/trash` et `/restore` exigeant l'écriture dans l'espace et la révision courante ; une transition non autorisée reçoit `422` et un objet en corbeille est refusé en écriture.
- Filtre d'état sur la recherche paginée (`state=active|archived|trashed|all`, actifs par défaut) et table d'audit conservant l'auteur et l'état avant/après de chaque transition.
- Client Windows : filtre d'état de l'inventaire et archivage, mise à la corbeille ou restauration de l'objet sélectionné.
- Tests MariaDB des transitions, du verrouillage par révision et de l'audit, test HTTP des droits et des refus, et vérifications client de l'archivage et du filtre d'état.
- Inventaire paginé par numéro (50 objets par défaut, maximum 100) et recherche littérale dans les noms, avec bouton de chargement des pages suivantes dans le client Windows.
- Tests MariaDB de la pagination, des recherches et des paramètres invalides ; vérification client de l'encodage de recherche et du curseur.
- Routes HTTP de transfert et d'historique des objets, avec droits sur les deux espaces, conflit de révision et protection des anciens espaces cités par l'historique.
- Client Windows : transfert de l'objet sélectionné vers un autre espace et affichage de son historique.
- Test MariaDB du transfert HTTP, des refus d'accès et de l'historique ; vérifications client du contrat de transfert.
- Premier parcours connecté : création et liste d'espaces, création et liste d'objets, lecture d'un objet, avec contrôle du jeton et des droits par espace.
- Écran Windows « Collection » pour choisir un espace et gérer un inventaire simple.
- Migration accordant explicitement la lecture aux propriétaires d'espaces existants, et test HTTP du parcours et de l'isolement des espaces.
- Client Windows : page Compte avec configuration de l'adresse du serveur, vérification de santé, connexion, liste et révocation des sessions. Le jeton est conservé uniquement en mémoire et n'est envoyé qu'au serveur configuré.
- Page Paramètres du client Windows : libellé de navigation en français, configuration et test de l'adresse du serveur, choix temporaire du thème Windows, clair ou sombre.
- Création d'espace établissant dans une même transaction l'espace, sa ligne de compteur d'inventaire et l'adhésion de son propriétaire.
- Adhésions avec droits explicites : ajout d'un membre, remplacement de ses droits, retrait et liste des membres.
- Refus d'une permission globale comme droit d'espace, d'un membre déjà présent, d'un membre qui modifierait ses propres droits et du retrait du propriétaire courant.
- Le propriétaire d'un espace ne reçoit aucun droit financier : la propriété n'implique que la lecture et l'écriture de collection.
- Tests d'extrémité du contrôle d'autorisation par espace, avec adhésion chargée depuis la base, y compris le cas d'un identifiant d'espace connu par un non-membre.
- Création d'objet avec attribution du numéro d'inventaire dans une transaction verrouillant le compteur de l'espace, et refus d'un nom vide ou dépassant 255 caractères.
- Transfert d'objet renuméroté dans l'espace de destination, avec conservation de l'ancien et du nouveau numéro dans l'historique et augmentation de la révision.
- Refus d'un transfert dont la révision est obsolète, d'une destination identique à la source et d'une destination sans compteur, sans modifier l'objet ni consommer de numéro.
- Lecture d'un objet, de son historique de transferts et du propriétaire courant d'un espace, en préparation des contrôles d'autorisation par espace.
- Tests d'intégration de concurrence : dix créations simultanées obtiennent dix numéros distincts, et deux transferts simultanés du même objet n'en laissent gagner qu'un.
- Aide de test partagée imposant l'ordre de suppression des tables, pour qu'une nouvelle table ne casse plus les autres suites par contrainte de clé étrangère.
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

- Aucune permission globale ne peut être enregistrée comme droit d'espace ; les tentatives sont refusées avant écriture.
- Un membre ne peut pas élargir ses propres droits, et le propriétaire ne peut pas être retiré de son espace.
- Les droits d'espace sont relus depuis la base à chaque opération, de sorte qu'un droit retiré cesse d'agir immédiatement.

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
