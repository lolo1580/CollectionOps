# Cahier des charges fonctionnel V1 — brouillon à valider

- Statut : brouillon ; D01 à D05 et D11 à D15 sont validées. Les autres règles restent proposées.
- Jalon : 01 — Cahier des charges fonctionnel.
- Références : [architecture](../architecture/overview.md), [frontière d'autorisation](../architecture/ADR-0002-authentication-boundary.md), [identifiants et sessions](../architecture/ADR-0003-local-credentials-and-sessions.md).

## Objectif et périmètre

CollectionOps permet de gérer des collections personnelles et partagées depuis un client Windows, avec un serveur central et une consultation ou modification hors ligne contrôlée. La V1 couvre les comptes et espaces, l'inventaire, les acquisitions et finances, les documents, l'historique, les échanges de données et la synchronisation. Le client Web, les galeries publiques, les connecteurs externes et l'identification automatique relèvent des versions ultérieures.

Les exigences ci-dessous décrivent des résultats attendus, sans figer le modèle de données ni les écrans. Les décisions encore ouvertes à la fin du document doivent être validées avant que les fonctionnalités concernées soient implémentées.

## Exigences et critères d'acceptation proposés

### Comptes, accès et espaces

| ID | Exigence | Critère d'acceptation vérifiable |
|---|---|---|
| F01 | Un utilisateur identifié accède à ses espaces autorisés. | Un utilisateur sans permission sur un espace ne peut en lire ni les données ni les documents, y compris via une URL ou un identifiant connu. |
| F02 | Un espace peut être privé ou partagé avec des droits explicites. | Seul le propriétaire invite par e-mail et choisit les droits ; lecture seule est présélectionnée, sans droit financier. Le lien à usage unique expire après sept jours, peut être révoqué ou réémis, et l'ancien lien cesse de fonctionner. Un droit retiré bloque les nouvelles opérations serveur. |
| F03 | Les droits financiers sont indépendants des droits sur l'inventaire. | La lecture ou la modification d'un objet n'accorde pas l'accès à ses montants financiers sans permission financière correspondante. |
| F04 | Un utilisateur consulte ses sessions et peut révoquer un appareil. | Après révocation, le jeton de cet appareil ne permet plus d'appeler une route protégée. |
| F05 | Les actions sensibles sont auditées. | Pour une modification de droit, suppression, restauration ou opération financière, l'historique conserve auteur, date, cible et nature de l'action. |

Scénarios de recette de F02 à conserver pour le premier lot :

1. Un membre qui n'est pas propriétaire ne peut ni créer, ni révoquer, ni réémettre une invitation.
2. Une invitation adressée à une personne sans compte peut être créée sans ouvrir un accès à l'espace.
3. Une invitation acceptée une fois, expirée après sept jours ou révoquée ne peut plus créer d'adhésion.
4. La réémission invalide le lien précédent, même si ses sept jours ne sont pas écoulés.
5. Une personne dont l'adresse vérifiée diffère de l'adresse invitée ne peut pas accepter l'invitation.
6. Une invitation créée sans modifier les choix présélectionnés accorde uniquement la lecture de collection ; elle ne donne accès à aucun montant financier.
7. Les droits accordés après acceptation correspondent aux choix explicites conservés avec l'invitation, même si les réglages par défaut changent ensuite.
8. Si le destinataire est déjà membre de l'espace, l'acceptation de l'invitation échoue sans modifier son adhésion ou ses droits.

### Inventaire et organisation

| ID | Exigence | Critère d'acceptation vérifiable |
|---|---|---|
| F06 | Un utilisateur autorisé crée, consulte, modifie et archive un objet de collection. | L'objet conserve un identifiant stable ; l'archivage ne le supprime pas de l'historique. |
| F07 | Les objets peuvent être organisés par catégories, séries, regroupements et emplacements. | Une recherche ou un filtre retrouve les objets selon ces relations, sans modifier leur identité. |
| F08 | Les fiches acceptent des champs adaptés à la collection et des références historiques ou techniques. | Les valeurs saisies sont restituées après sauvegarde et restent associées au bon objet. |
| F08a | Les catégories sont propres à un espace, imbriquées et multiples par objet ; leurs champs texte, nombre et date sont hérités par les sous-catégories. | Un champ d'ancêtre apparaît une seule fois même si l'objet est classé dans plusieurs branches ; les catégories d'un autre espace sont refusées. |
| F08b | Un transfert d'objet déjà classé exige le choix explicite de catégories de destination. | Un transfert sans choix est refusé ; les anciennes valeurs restent historiques sans être révélées dans l'espace de destination. |
| F09 | Les transferts d'emplacement sont traçables. | L'emplacement courant et les transferts passés peuvent être consultés par un utilisateur autorisé. |
| F10 | Les suppressions suivent une corbeille et une politique de restauration. | Un élément supprimé n'apparaît plus dans la vue courante ; sa restauration, si autorisée, restitue ses relations conservées. |

### Acquisitions et finances

| ID | Exigence | Critère d'acceptation vérifiable |
|---|---|---|
| F11 | Une liste d'envies suit les recherches, offres et vendeurs. | Plusieurs offres peuvent être liées à une même envie et comparées selon les montants enregistrés. |
| F12 | Une acquisition peut provenir d'un achat, cadeau, échange, don ou mode personnalisé. | Le mode choisi et les informations applicables sont conservés dans l'historique de l'objet. |
| F13 | Les prix, frais et estimations conservent leur devise, source et date. | Une nouvelle estimation n'efface pas les précédentes ; l'absence d'estimation peut être représentée selon la règle financière validée. |
| F14 | Les budgets et valorisations peuvent être consultés selon les permissions financières. | Chaque total est reproductible à partir des lignes, taux et dates utilisés ; un utilisateur sans droit financier ne voit aucun montant. |
| F15 | Les ventes, échanges et sorties sont enregistrés. | Le résultat réalisé et le changement d'état de l'objet sont cohérents avec les montants et opérations enregistrés. |

### Documents, échanges et continuité

| ID | Exigence | Critère d'acceptation vérifiable |
|---|---|---|
| F16 | Un objet peut recevoir des photographies et documents versionnés. | Les versions antérieures restent consultables selon les droits ; une copie dédupliquée ne mélange jamais les permissions des objets. |
| F17 | Les imports et exports disposent d'un aperçu et d'un compte rendu. | Les lignes rejetées indiquent une cause ; une réexécution ne crée pas silencieusement des doublons. |
| F18 | Le client Windows permet les parcours essentiels en ligne et hors ligne. | Les modifications hors ligne sont mises en attente, reprises après reconnexion et les conflits sont présentés selon une règle documentée. |
| F19 | Les droits révoqués sont appliqués lors de la reconnexion. | Le client cesse de synchroniser les données devenues interdites et suit la politique de purge locale validée. |
| F20 | Les sauvegardes permettent une restauration contrôlée. | Une restauration complète et une restauration sélective sont vérifiables sur un jeu de données de recette. |

## Contraintes transversales déjà décidées

- Le client utilise l'API HTTPS versionnée ; seul le backend accède à MariaDB.
- Le backend refuse les routes protégées sans identité vérifiée et contrôle les permissions effectives, sans accorder de droit implicite à un rôle.
- Les mots de passe locaux utilisent Argon2id ; les secrets de session sont opaques et seules leurs empreintes sont destinées à être persistées.
- La copie locale est une réplique partielle chiffrée. Le protocole de synchronisation doit être défini avant son implémentation.
- Les documents sont adressés par métadonnées indépendantes du stockage physique.

## Décisions métier

### Décisions validées pour le premier lot

| ID | Règle validée | Conséquence immédiate |
|---|---|---|
| D01 | Un premier administrateur est créé lors de l'installation ; les autres comptes arrivent par invitation. | Aucun parcours d'inscription libre dans la V1. |
| D02 | Chaque espace a un propriétaire ; lecture et modification sont attribuées explicitement, avec droits financiers séparés. | Les droits doivent être évalués dans le contexte de l'espace. La délégation de l'administration reste à préciser. |
| D03 | Un objet appartient à un seul espace à la fois ; un transfert conserve son identité et son historique. | Le transfert change l'espace courant sans créer de nouvel objet. |
| D04 | Un objet exige un nom ; le serveur attribue un identifiant stable et un numéro d'inventaire séquentiel, unique dans l'espace. | L'attribution du numéro doit être atomique et sûre en cas de créations concurrentes. Les autres champs obligatoires restent à préciser. |
| D05 | Un objet est `active`, `archived` ou `trashed` (corbeille). Chaque état est réversible : l'archivage masque sans supprimer, la corbeille retire de la vue courante et peut être restaurée. Aucune suppression physique n'existe en V1. Un objet en corbeille est en lecture seule jusqu'à sa restauration. | Aucun numéro n'est réutilisé, l'identifiant et l'historique sont conservés quel que soit l'état. La suppression définitive, réservée au propriétaire, reste à concevoir hors V1. |
| D11 | Lors d'un transfert, l'objet reçoit un nouveau numéro dans l'espace de destination ; l'ancien numéro reste dans l'historique. | Le numéro ne change pas l'identité stable de l'objet et les deux valeurs figurent dans l'enregistrement du transfert. |
| D12 | Une invitation est adressée par e-mail, y compris à une personne qui n'a pas encore de compte. | L'acceptation doit vérifier que le compte contrôle l'adresse invitée ; le compte peut être créé pendant ce parcours. |
| D13 | Seul le propriétaire d'un espace peut inviter ; le lien est à usage unique et expire après sept jours. Le propriétaire peut le révoquer ou le réémettre, ce qui invalide le lien précédent. | La durée, la consommation, la révocation et la réémission sont contrôlées côté serveur et auditées. |
| D14 | Le propriétaire choisit les droits dans l'invitation ; la présélection est la lecture seule de collection, sans droit financier. | Les droits choisis sont conservés avec l'invitation et appliqués seulement après son acceptation. |
| D15 | Une invitation destinée à un membre déjà présent ne peut pas être acceptée. | Les droits du membre restent inchangés ; leur modification relève d'un parcours distinct et audité. |

### Décisions restant à valider, par ordre de dépendance

| ID | Décision attendue | Conditionne |
|---|---|---|
| D06 | Quelle visibilité donner aux prix et documents lorsque l'objet est partagé ? | Matrice des permissions, contrats API. |
| D07 | Quelles règles de devise, taux historique, arrondi et valeur sans estimation appliquer ? | Calculs, budgets, rapports financiers. |
| D09 | Quels conflits hors ligne peuvent être fusionnés automatiquement et lesquels exigent un choix humain ? | Protocole de synchronisation. |
| D10 | Quels formats d'import/export et quelles limites de taille documentaire retenir pour la V1 ? | API, stockage, recette. |

### Durées de session validées (ex-D08)

Décision prise le 2026-09-22, détaillée dans [ADR-0003](../architecture/ADR-0003-local-credentials-and-sessions.md) :

- un jeton par appareil, jamais partagé entre clients ;
- un nouveau jeton à chaque connexion, jamais réutilisé ;
- verrouillage du client après 15 minutes, 30 minutes, 1 heure ou jamais d'inactivité ; le délai court depuis la dernière activité ;
- plafond serveur de sept jours, appliqué par le schéma, y compris pour le réglage « jamais ».

La purge de la copie locale reste à définir avant le mode hors ligne.

## Première tranche proposée après validation

1. Établir la matrice des permissions par opération et par espace ; préciser la délégation du propriétaire.
2. Définir le modèle conceptuel des comptes, espaces, adhésions et objets, avec cardinalités et règles de suppression.
3. Décrire les contrats API des parcours compte/espace/objet avant la persistance et le client réseau.
4. Transformer F01 à F06 en scénarios de recette, comprenant au minimum les refus d'accès entre espaces et la révocation d'une session.

La validation de ce brouillon doit indiquer les exigences retenues pour la V1, celles à reporter, et les réponses aux décisions ouvertes. Elle ne vaut pas validation automatique des choix de sécurité ou de synchronisation restant à concevoir.
