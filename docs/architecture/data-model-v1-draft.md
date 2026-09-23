# Modèle conceptuel du premier lot V1 — brouillon

- Statut : proposition technique fondée sur D01 à D04 et D11 à D15 du [cahier des charges](../product/cahier-des-charges-v1.md).
- Périmètre : comptes, espaces, appartenance et objets. Les migrations MariaDB couvrent désormais aussi l'identité e-mail, les sessions et les invitations de base. Le choix de droits à l'invitation et la suppression définitive demandent encore des décisions complémentaires.

## Entités et cardinalités

| Entité | Identité et attributs minimaux | Relations |
|---|---|---|
| Compte | Identifiant stable, nom affiché, adresse e-mail vérifiée, état. | Peut être propriétaire de plusieurs espaces et membre de plusieurs autres. Le mode de connexion reste à préciser. |
| Espace | Identifiant stable, nom, propriétaire, état. | Possède exactement un propriétaire courant et zéro à plusieurs objets. |
| Adhésion | Espace, compte, permissions explicites, état. | Lie un compte à un espace ; au plus une adhésion active par couple. |
| Invitation | Espace, adresse e-mail destinataire, auteur, empreinte du jeton, dates de création, expiration, acceptation et révocation. | Peut précéder le compte destinataire ; seule une invitation active peut créer une adhésion. |
| Droits d'invitation | Invitation, permission d'espace explicitement choisie. | Le jeu de droits est copié vers l'adhésion à l'acceptation ; il ne donne aucun accès avant. |
| Objet | Identifiant stable, espace courant, nom non vide, numéro d'inventaire attribué par le serveur, révision positive, état. | Appartient à exactement un espace courant. |
| Transfert | Objet, espace source, espace cible, auteur, date, références des numéros d'inventaire. | Retrace chaque changement d'espace sans changer l'identité de l'objet. |

Le premier administrateur est provisionné à l'installation. Aucun compte public ne peut être créé par une route d'inscription libre. Une invitation peut cibler une adresse sans compte. Elle ne devient une adhésion qu'après acceptation par un compte ayant vérifié cette adresse ; le compte peut être créé pendant ce parcours. Le seul fait de connaître l'adresse ne suffit pas.

## Contraintes d'intégrité envisagées

1. Les clés des comptes, espaces et objets sont stables et ne sont jamais réutilisées.
2. L'espace courant d'un objet existe et n'est jamais nul.
3. Le nom d'un objet, après suppression des espaces de bord, contient au moins un caractère. La longueur maximale reste à fixer avant la migration SQL.
4. Le couple `(espace courant, numéro d'inventaire)` est unique en base, y compris sous créations concurrentes. Le serveur attribue un numéro séquentiel propre à l'espace dans la même transaction que l'objet. Les numéros attribués ne sont pas réutilisés ; des trous sont possibles.
5. Un transfert met à jour l'espace courant, attribue le prochain numéro de la destination et inscrit l'ancien et le nouveau numéro dans l'historique, le tout dans une transaction unique.
6. La révision d'un objet commence à `1` et augmente à chaque modification. Une commande qui présente une ancienne révision échoue sans écraser l'état courant.
7. Les permissions d'une adhésion sont explicites. Les droits financiers ne sont pas déduits des droits de lecture ou de modification de l'inventaire.
8. Les règles d'archivage, de corbeille et de suppression physique restent hors de cette première migration tant que D05 n'est pas tranchée. Aucun chemin de suppression n'est prévu dans le premier lot de persistance.
9. Une invitation conserve l'adresse ciblée et l'identité de son auteur. Un compte accepté ne reçoit aucun droit tant que le contrôle de l'adresse et la validité de l'invitation ne sont pas établis.
10. L'adresse e-mail est unique par compte non nul et dispose d'un horodatage de vérification. Le premier administrateur est marqué comme vérifié, puisqu'il est créé par l'exploitant. L'acceptation devra vérifier dans une transaction que l'adresse du compte est vérifiée et correspond à celle de l'invitation ; le schéma seul n'établit pas cette preuve. La normalisation retenue ne modifie pas la partie locale de l'adresse, donc la règle de comparaison des invitations reste à décider.
11. Une invitation n'est active que si elle n'est ni acceptée, ni révoquée, ni expirée. Son jeton aléatoire est transmis uniquement au destinataire ; seule son empreinte est conservée en base. Le lien expire sept jours après sa création et ne peut être consommé qu'une fois.
12. Seul le propriétaire de l'espace peut créer, révoquer ou réémettre une invitation. La réémission révoque l'ancienne invitation et crée un nouveau jeton dans une même transaction. Les événements doivent être audités avant mise en service.
13. La création d'invitation présélectionne `collections_read` uniquement. Le propriétaire peut choisir d'autres permissions d'espace ; aucune permission financière n'est présélectionnée. Les permissions globales d'administration ne sont pas des droits d'espace invitables.

## Frontière d'autorisation à faire évoluer

Le `Principal` actuel expose des permissions effectives sans contexte d'espace. Ce modèle suffit au contrat de session initial mais ne permet pas, seul, de décider l'accès à un objet. Chaque opération future sur un espace ou un objet devra vérifier l'adhésion et les droits de cet espace côté serveur, puis appliquer les permissions globales pertinentes. Un identifiant d'objet connu ne doit jamais contourner cette vérification.

| Opération envisagée | Autorisation minimale à confirmer |
|---|---|
| Voir un espace ou un objet | Adhésion à l'espace et lecture de collection. |
| Créer ou modifier un objet | Adhésion à l'espace et écriture de collection. |
| Transférer un objet | Droit d'écriture sur la source et la destination ; décision du propriétaire à préciser. |
| Voir ou modifier des montants | Permission financière explicite pour l'espace concerné. |
| Inviter, révoquer ou réémettre une invitation | Être le propriétaire courant de l'espace. |
| Retirer un membre ou modifier ses droits | Être le propriétaire courant de l'espace. Un membre ne peut pas modifier ses propres droits, et le propriétaire ne peut pas être retiré. |

La forme du jeton et le contenu du `Principal` ne doivent pas être utilisés comme source unique des adhésions : leur révocation et leurs changements doivent prendre effet selon une politique de session validée.

**État : implémenté et couvert par des tests.** `create_space`, `membership`, `add_member`, `set_member_permissions`, `remove_member` et `space_members` existent, et les tests appellent réellement `require_space_permission` après avoir chargé l'adhésion depuis la base. Aucune route HTTP correspondante n'est encore exposée.

La création d'un espace établit ensemble l'espace, la ligne de compteur et l'adhésion du propriétaire avec `collections_read` et `collections_write`. Le propriétaire ne reçoit **aucun** droit financier, et aucune permission globale ne peut être stockée comme droit d'espace.

## Questions restantes pour les extensions

- Faut-il étendre la politique actuelle (ASCII, domaine mis en minuscules, unicité insensible à la casse) pour gérer les adresses internationales, et comment vérifier les comptes préexistants non vérifiés ?
- Le propriétaire peut-il déléguer la gestion des membres et des droits financiers, hors création d'invitations réservée au propriétaire ?
- Quelles sont les règles de suppression et de conservation des espaces, objets, comptes et invitations ?

Ces réponses guident les migrations et routes complémentaires. La première tranche d'invitations utilise SMTP et un lien à coller dans le client Windows ; les droits sont pour l'instant fixés à `collections_read`. Les transactions et contraintes ci-dessus restent des critères de revue pour les extensions à venir.

La [première migration MariaDB](../../backend/migrations/202609220001_core.sql) fixe les contraintes du noyau comptes, espaces et inventaire, la [deuxième](../../backend/migrations/202609220002_account_credentials.sql) ajoute l'adresse e-mail et les mots de passe, la [troisième](../../backend/migrations/202609220003_account_sessions.sql) crée les sessions, et la [migration des invitations](../../backend/migrations/202609230001_space_invitations.sql) ajoute les liens, droits et événements d'audit. SQLx les applique au démarrage dès que `COLLECTIONOPS_DATABASE_URL` est défini. Le [brouillon SQL](../schema/invitations-draft.sql) reste une référence de conception, non une migration à exécuter.

## Invitation et acceptation

Le serveur génère un jeton opaque de 256 bits avec le générateur cryptographique du système, l'envoie au destinataire et ne conserve que son empreinte SHA-256. Le lien `collectionops://invite/...` contient l'identifiant de l'invitation, le jeton et l'origine HTTPS `COLLECTIONOPS_PUBLIC_URL` configurée côté serveur ; il n'utilise jamais l'en-tête `Host` fourni par la requête. Le client vérifie que son adresse de serveur correspond à celle du lien et transmet le jeton dans le corps de la requête d'acceptation, pas dans l'URL HTTP. Les journaux ne contiennent ni jeton ni URL complète.

À l'acceptation, une transaction verrouille l'invitation, compare le jeton, vérifie son expiration et ses états, puis vérifie que le compte contrôle l'adresse visée. Si le compte n'existe pas, la possession du lien reçu par e-mail permet sa création et marque l'adresse vérifiée dans la même transaction. Si le compte est déjà membre, l'acceptation échoue sans modifier ses droits. Sinon, la transaction crée l'adhésion, copie les droits conservés dans `space_invitation_grants` vers `space_permission_grants` et marque l'invitation acceptée ensemble. Une seconde tentative sur le même lien échoue sans ajouter de droits. Les droits d'invitation ne sont jamais pris depuis les valeurs par défaut courantes au moment de l'acceptation.

La création et la réémission verrouillent la ligne de l'espace pour sérialiser les invitations vers la même adresse. La réémission révoque l'invitation active précédente, insère une nouvelle invitation et journalise l'opération dans la même transaction. La révocation prend effet avant tout envoi d'un nouveau lien. Le service ne doit pas accepter un lien expiré même si un nettoyage différé laisse sa ligne en base.

## Attribution des numéros dans une transaction

**État : implémenté et couvert par des tests.** `create_item` et `transfer_item` appliquent les règles ci-dessous ; la création d'espaces qui doit insérer la ligne de compteur reste à écrire.

La table `inventory_counters` contient une ligne par espace, créée avec l'espace et initialisée à `1`. La valeur `next_number` désigne le prochain numéro libre, jamais un numéro déjà utilisé. La création d'un objet suit cet ordre dans une seule transaction : vérifier les droits sur l'espace, verrouiller sa ligne de compteur avec `SELECT ... FOR UPDATE`, lire `next_number`, l'incrémenter, insérer l'objet avec le numéro lu, puis valider. Un échec annule aussi l'incrément. L'unicité `(space_id, inventory_number)` protège la règle même si un appelant commet une erreur.

Le transfert verrouille d'abord l'objet et vérifie sa révision attendue et son espace courant, puis les droits sur la source et la destination. Il verrouille ensuite le compteur de destination, réserve son prochain numéro, modifie l'espace et le numéro courants ainsi que la révision de l'objet, inscrit une ligne `inventory_transfers` avec les deux espaces et les deux numéros, puis valide la transaction. Deux transferts simultanés du même objet sont ainsi sérialisés ; la seconde commande reçoit un conflit de révision. En cas d'interblocage entre opérations concurrentes, l'appelant doit rejouer la transaction complète ; aucune opération partielle ne doit être considérée comme réussie.

Les anciens numéros ne sont pas réutilisés. Le numéro d'inventaire sert à la lecture humaine dans un espace donné ; l'identifiant stable de l'objet sert aux liens, à l'API et à l'historique. Le format affiché peut être précisé côté client sans modifier la séquence stockée.
