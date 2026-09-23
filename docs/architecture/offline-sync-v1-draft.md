# Synchronisation hors ligne V1 — cadrage provisoire

- Statut : proposition à valider, non implémentée.
- Périmètre initial : client Windows, espaces et objets de collection. Les montants financiers, documents et transferts entre espaces attendent des règles propres.
- Références : [cahier des charges V1](../product/cahier-des-charges-v1.md), [autorisation par espace](ADR-0004-space-authorization.md), [contrat API actuel](../api/v1-core-draft.md).

## Invariants de sécurité et de cohérence

1. MariaDB reste la source de vérité. La copie locale ne prouve jamais une adhésion ni un droit ; le serveur les recontrôle pour chaque commande envoyée à la reconnexion.
2. Une session expirée ou révoquée bloque l'envoi et la réception. Le client ne doit pas interpréter l'absence de réseau comme une autorisation renouvelée.
3. La copie locale est limitée aux espaces et champs explicitement autorisés lors du dernier échange réussi. Les données financières et documents ne sont pas copiés par défaut avec `collections_read`.
4. La file locale conserve les opérations et leurs identifiants, pas les secrets de session dans les charges utiles ni les URL. Les journaux ne contiennent pas le contenu des objets ou des documents.
5. L'interface distingue clairement les données confirmées par le serveur, les modifications en attente et les conflits. Elle n'annonce jamais « synchronisé » avant un accusé de réception durable.

## Échange proposé

Le client conserve un curseur opaque par espace et type de données. Après authentification, il demande les changements autorisés depuis ce curseur ; le serveur vérifie les droits avant de retourner une page ordonnée et le curseur suivant. Le curseur ne vaut ni identité ni autorisation. Une réponse indiquant un curseur trop ancien déclenche une reconstruction contrôlée de la copie de l'espace, sans effacer la file des opérations non envoyées.

Chaque commande hors ligne reçoit un identifiant stable généré une fois par le client, l'identifiant de l'objet, l'espace connu, la révision serveur de départ et les champs modifiés. Lors de la reconnexion, le serveur vérifie la session et les droits courants, puis applique la commande et mémorise son identifiant dans la même transaction. Un rejeu après interruption retourne le résultat déjà enregistré : il ne crée pas un second objet et n'incrémente pas la révision deux fois. Les commandes sont envoyées dans l'ordre de dépendance ; une panne n'efface pas les commandes non accusées.

Le protocole devra spécifier les bornes de taille, la durée de conservation des accusés et des changements, la pagination, les suppressions logiques, les horodatages et la reprise après restauration de sauvegarde. Aucun nouvel endpoint de synchronisation ne doit être annoncé comme disponible avant migrations et tests correspondants.

## Conflits proposés

Le renommage en ligne utilise déjà `expected_revision` et renvoie `409` si la version serveur a changé. La même règle s'appliquerait à une commande hors ligne : ne jamais écraser silencieusement le serveur. Le client conserverait sa version locale, afficherait la version serveur et demanderait à l'utilisateur de choisir ou de ressaisir. Une fusion automatique ne serait permise qu'après définition champ par champ d'opérations commutatives ; aucune n'est approuvée pour l'instant. Un transfert entre espaces ne serait pas mis en file dans la première tranche, car il dépend des droits courants sur les deux espaces et de la numérotation serveur.

## Révocation et copie locale

À la reconnexion, une adhésion ou permission retirée interdit immédiatement les nouvelles lectures et écritures de cet espace. Le client doit aussi cesser d'en afficher les données en cache. Le devenir de la copie locale chiffrée et des commandes en attente reste une décision produit explicite : purge immédiate, conservation temporaire contrôlée ou export chiffré pour récupération. Tant que cette politique n'est pas validée et testée, la modification hors ligne ne doit pas être activée.

## Préconditions avant implémentation

- Valider la politique de purge et de traitement des opérations en attente après révocation.
- Définir les champs d'objet synchronisables et leurs règles de conflit, puis les durées de conservation côté serveur.
- Choisir et documenter la gestion des clés du stockage SQLite chiffré sur Windows, la déconnexion et la perte d'appareil.
- Ajouter les migrations du journal de changements et des commandes idempotentes, puis les tests de rejeu, ordre, conflit, droits retirés, sauvegarde/restauration et copie locale.
- Effectuer une revue de sécurité du protocole et de la copie locale avant exposition aux utilisateurs.
