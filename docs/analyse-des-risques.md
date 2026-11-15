---
title: Polyptyque - Analyse des risques et reprise après incident
aliases: ["Analyse des risques Polyptyque", "Reprise après incident"]
tags: [polyptyque, projet, fiabilite, analyse-des-risques]
created: 2026-10-21
status: active
type: projet
---

# Polyptyque - Analyse des risques et reprise après incident

> [!abstract] Résumé
> Inventaire de ce qui peut mal tourner pendant un enregistrement : défaillance de Polyptyque, de Windows ou du matériel, des programmes capturés, de l'encodage ou du disque, et erreurs de manipulation.
> Pour chaque cas : ce que fait Polyptyque **aujourd'hui** (vérifié dans le code de la branche `dev` au 2026-10-21, ou signalé « à vérifier »), ce qui manque, et la proposition.
> Les propositions sont regroupées en trois lots par priorité, à valider avant toute réalisation.
> Complète le [[cahier-des-charges|cahier des charges]].

> [!success] Mise à jour du 2026-10-22
> Le **lot 1** est réalisé (à tester sur la machine de référence), ainsi que, des lots 2 et 3 : conversion MP4 sous nom temporaire (A6), mise en veille (B3), relance après un redémarrage (B2), bascule sur le processeur (D2), nettoyage des journaux (A7). Les colonnes « Aujourd'hui » décrivent l'état **avant** ces travaux ; le [[#Plan d'action proposé|plan d'action]] indique ce qui est fait.

> [!success] Mise à jour du 2026-10-23
> Programmation des enregistrements ajoutée (voir le [[cahier-des-charges#Programmation des enregistrements|cahier des charges]]) : nouveaux scénarios B10, B11 et E11. Le redémarrage par Windows Update (B2) est aussi couvert par l'option « Lancer Polyptyque au démarrage de Windows ».

> [!success] Mise à jour du 2026-11-15
> Revue de fiabilité du code : B3, D2, A4, A5, A6, D1 et D4 complétés (voir le [[#Plan d'action proposé|plan d'action]]) ; nouveaux scénarios A10, A11, C6, C7, D8 et D9, déjà couverts.

## Table des matières

1. [[#Principes]]
2. [[#Méthode]]
3. [[#A - Polyptyque lui-même]]
4. [[#B - Windows et le matériel]]
5. [[#C - Programmes capturés]]
6. [[#D - Encodage et disque]]
7. [[#E - Erreurs de manipulation]]
8. [[#Déjà couvert]]
9. [[#Plan d'action proposé]]
	1. [[#Lot 1 - Reprise après incident]]
	2. [[#Lot 2 - Situations prévisibles]]
	3. [[#Lot 3 - Confort et prévention]]
10. [[#Tests à mener sur la machine de référence]]
11. [[#Questions ouvertes]]
12. [[#Sources]]

---

## Principes

> [!important] Cinq règles
> 1. **Ne jamais perdre ce qui a été enregistré.** Un incident peut couper un enregistrement, jamais abîmer ce qui est déjà sur le disque.
> 2. **Ne jamais échouer en silence.** Tout problème se voit dans l'interface, avec une explication et, si possible, une action pour le résoudre.
> 3. **Se rétablir seul quand c'est sûr**, sinon rendre la reprise possible en un clic.
> 4. **Ne rien faire de destructif ou d'inattendu sans accord** (supprimer, déplacer une fenêtre, écraser un fichier).
> 5. **Tout noter dans le journal**, pour comprendre après coup.

## Méthode

Chaque scénario porte un identifiant (A1, B3…) repris dans le plan d'action.

| Colonne | Valeurs |
|---|---|
| Probabilité | faible, moyenne, forte |
| Gravité | faible (gêne), moyenne (perte de temps), forte (perte d'une partie d'un enregistrement), critique (perte de données ou de réglages) |
| Priorité | P1 (lot 1), P2 (lot 2), P3 (lot 3), — (déjà couvert) |

---

## A - Polyptyque lui-même

| N° | Scénario | Prob. | Grav. | Aujourd'hui | Proposition | Prio. |
|---|---|---|---|---|---|---|
| A1 | Erreur interne de l'interface pendant un enregistrement | faible | forte | Polyptyque se ferme d'un coup. Les FFmpeg reçoivent la fin de leurs flux et ferment proprement leurs fichiers (probable, **à vérifier**). Au redémarrage, rien ne signale l'interruption et rien n'est repris | Journal de session (voir [[#Lot 1 - Reprise après incident]]) : au redémarrage, bilan des fichiers et bouton « Reprendre l'enregistrement » | P1 |
| A2 | Erreur interne dans le fil de capture ou d'écriture d'une source | faible | forte | Le fil s'arrête **sans prévenir** : la source peut continuer avec le son seul, ou plus rien, alors que la carte affiche encore « Enregistre » | Chaque fil protégé : l'erreur est affichée sur la carte et la capture de la source est relancée, comme l'encodeur | P1 |
| A3 | Interface figée | très faible | moyenne | L'enregistrement continue (le moteur est indépendant), mais ne peut être arrêté qu'en fermant Polyptyque de force ; les fichiers sont alors fermés par FFmpeg (probable, **à vérifier**) | Arrêt d'urgence par le raccourci global, traité hors de l'interface | P3 |
| A4 | Deux Polyptyque ouverts en même temps | moyenne | moyenne | Rien ne l'empêche : réglages écrasés par l'un ou l'autre, raccourcis refusés au second, fichiers de même nom (FFmpeg refuse d'écraser : une source échoue) | Instance unique : un second lancement ramène la fenêtre déjà ouverte | P1 |
| A5 | Fichier de réglages abîmé (scènes, sources) | faible | critique | Copie dans `config.toml.invalide`, puis réglages vierges ; la sauvegarde suivante remplace l'original. Écriture atomique, mais sans forçage sur le disque : une coupure de courant juste après une sauvegarde peut laisser un fichier vide | Écriture forcée sur le disque ; cinq sauvegardes tournantes ; restauration automatique de la dernière bonne, avec message | P1 |
| A6 | Conversion MP4 interrompue (plantage, coupure) | faible | moyenne | Le MP4 partiel garde son nom définitif et semble valide alors qu'il est tronqué | Écriture sous un nom temporaire, renommé seulement après vérification ; restes supprimés au démarrage | P2 |
| A7 | Journaux qui s'accumulent | forte | faible | Un journal par lancement et un dossier par session, jamais supprimés | Suppression automatique après 30 jours | P3 |
| A8 | Mise à jour interrompue ou bloquée | faible | moyenne | Velopack 1.2.161 remplace le dossier du programme en deux temps : l'ancien est mis de côté, puis le nouveau prend sa place. Si le premier temps échoue (fichier verrouillé), l'ancienne version est relancée. Si le second échoue (par exemple un antivirus qui analyse les nouveaux fichiers), la copie mise de côté est effacée : Polyptyque ne démarre plus et doit être réinstallé ; réglages, scènes et profils, rangés ailleurs, sont conservés. Vérifié dans le code de Velopack, qui a corrigé ce défaut le 2026-10-10, pour l'instant seulement dans une préversion | Passer à la première version stable de Velopack qui contient ce correctif (pas de préversion) | P2 |
| A10 | FFmpeg de conversion, de réparation ou d'essai figé, qui survit à Polyptyque fermé ou planté | faible | faible | Le processus reste en mémoire et garde le fichier ouvert | Objet tâche de Windows fermé avec Polyptyque : ces FFmpeg s'arrêtent avec lui, plantage compris. Jamais ceux d'un enregistrement, qui doivent finir leurs fichiers seuls (réalisé le 2026-11-15) | — |
| A11 | Installateur ou maintenance qui demande la fermeture de Polyptyque (Gestionnaire de redémarrage) pendant un enregistrement | faible | forte | Traité comme une fin de session : enregistrement arrêté, fin du programme marquée normale alors qu'il reste ouvert | Refus pendant un enregistrement, qui continue ; fermeture ordinaire sinon (réalisé le 2026-11-15) | — |
| A9 | Mémoire qui grimpe sur 12 h | faible | forte | Mesuré pendant le test d'endurance du 2026-10-21 (14 h 15, trois sources) : environ 415 Mo pour Polyptyque et 195 Mo par FFmpeg, sans croissance constatée | Rien à ajouter | — |

## B - Windows et le matériel

| N° | Scénario | Prob. | Grav. | Aujourd'hui | Proposition | Prio. |
|---|---|---|---|---|---|---|
| B1 | Coupure de courant, écran bleu | faible | critique | Fichiers MKV lisibles (écrits par blocs d'une seconde) ; la fin est perdue, y compris ce que Windows n'avait pas encore écrit sur le disque (non mesuré). Fichiers sans index : le déplacement dans la vidéo est lent en montage. Rien n'est signalé au redémarrage | Au redémarrage : session interrompue détectée (A1), fichiers **réparés sans réencodage** (index reconstruit), reprise proposée | P1 |
| B2 | Redémarrage forcé par Windows Update | moyenne | forte | Blocage avec explication dans l'écran d'arrêt ; si l'arrêt est forcé, fichiers finalisés. Pas de reprise après le redémarrage. Windows Update ne redémarre automatiquement qu'en dehors des « heures d'activité » (8 h-17 h par défaut, plage réglable jusqu'à 18 h), sauf échéance imposée par une entreprise ; Microsoft écrit désormais « dans la mesure du possible » | **Prévenir** : conseiller de régler les heures d'activité sur la plage des sessions. **Reprendre** : bilan et reprise au lancement suivant (journal de session, lot 1). **Relancer** : demander à Windows de relancer Polyptyque après le redémarrage (`RegisterApplicationRestart`) ; Windows ne le fait que si l'utilisateur a activé la relance des applications à la connexion, désactivée par défaut, et la doc ne dit pas si Windows Update la déclenche (à tester) | P2 |
| B3 | Mise en veille demandée (capot fermé, bouton, menu) | moyenne | forte | La mise en veille **automatique** est empêchée pendant l'enregistrement ; celle-ci ne peut pas l'être (documenté par Microsoft). Polyptyque n'est pas prévenu. Son horloge continue d'avancer pendant la veille (documenté) : au réveil, les fichiers restent calés entre eux, et la durée de la veille y est comblée par une image figée et du silence. D'après le code (non testé) : l'encodeur doit ensuite traiter d'un coup toutes les images de la veille (216 000 pour une heure à 60 images/s) ; pendant ce rattrapage, qui peut durer plusieurs minutes, l'image enregistrée reste figée alors que le son est normal | Être prévenu par Windows quelques secondes avant la veille (inscription nécessaire sur les portables récents en « veille moderne ») : fichiers fermés proprement ; au réveil, reprise proposée ou automatique dans une nouvelle partie, sans combler la veille d'images figées | P2 |
| B4 | Écran débranché avec des fenêtres capturées (vu en test le 2026-10-21) | moyenne | moyenne | Les fenêtres passent « Réduites » : image figée (noire ici), son conservé, reprise dès leur retour | Bouton « Rétablir la fenêtre » sur la carte (déjà prévu) | P1 |
| B5 | Verrouillage de la session (Win+L) | moyenne | forte | L'écran reste allumé pendant l'enregistrement, donc pas de verrouillage automatique. Capture pendant un verrouillage manuel : **à vérifier** | Selon le test : avertissement, ou reprise automatique au déverrouillage | test |
| B6 | Redémarrage du pilote graphique | faible | forte | Probable (non testé) : capture perdue puis rattachée, encodeur relancé dans une nouvelle partie | Rien à ajouter si le test le confirme | test |
| B7 | Changement de sortie audio (casque branché, enceintes) | forte | moyenne | Le son est capté par programme et non par sortie ; effet d'un changement de sortie : **à vérifier** | Selon le test | test |
| B8 | Batterie critique | faible | forte | Arrêt non bloquable (voulu) ; finalisation tentée | Rien à ajouter | — |
| B9 | Changement d'heure (été/hiver, fuseau) | forte | faible | Horloge interne indépendante de l'heure affichée ; seuls les noms de fichiers utilisent l'heure de départ. Programmations : créneaux calculés sur l'heure affichée, heure inexistante décalée, heure double prise à sa première occurrence (vérifié par des tests avec le fuseau Europe/Paris) | Rien à ajouter | — |
| B10 | Enregistrement programmé manqué : Polyptyque fermé, ordinateur en veille ou redémarré à l'heure prévue | moyenne | forte | Programmation ajoutée le 2026-10-23 : veille automatique et extinction de l'écran empêchées tant qu'une programmation est active ; lancement au démarrage de Windows (option) ; au retour pendant le créneau, question avec compte à rebours de deux minutes, enregistrement lancé sans réponse ; plantage ou arrêt de Windows pendant un enregistrement programmé : même question au retour | Plus tard : lancement par Windows à l'heure prévue (Planificateur de tâches), sortie de veille automatique | — |
| B11 | Enregistrement programmé qui démarre session verrouillée, écran éteint (connexion automatique de Windows après une mise à jour, par exemple) | moyenne | forte | Capture dans ces conditions : **à vérifier** | Selon le test ([[protocole-test-programmation]]) | test |

## C - Programmes capturés

| N° | Scénario | Prob. | Grav. | Aujourd'hui | Proposition | Prio. |
|---|---|---|---|---|---|---|
| C1 | Navigateur ou application fermé, planté, ou redémarré par une mise à jour | forte | forte | Image noire et silence, puis rattachement automatique au retour. **Mais** pendant l'enregistrement, aucun bouton pour rouvrir la fenêtre, et la relance automatique d'une application ne joue qu'au démarrage | Bouton « Rouvrir » sur les cartes « Fenêtre perdue » ; relance automatique en cours d'enregistrement pour les sources qui l'autorisent | P1 |
| C2 | Fenêtre réduite par erreur | forte | moyenne | Image figée, avertissement « Réduite (image figée) » | Bouton « Rétablir la fenêtre » (B4) | P1 |
| C3 | Profil de navigateur supprimé à la main | faible | moyenne | Recréé vide **sans avertissement** : connexions à refaire | Message « Profil introuvable : il faudra te reconnecter » | P3 |
| C4 | Navigateur désinstallé ou déplacé | faible | moyenne | Message d'échec de lancement au démarrage de l'enregistrement | Proposer de choisir le nouvel emplacement | P3 |
| C5 | Jeu en plein écran exclusif, contenu protégé | moyenne | moyenne | Image noire (limite connue, documentée) | Rien à ajouter | — |
| C6 | Deux sources d'un même programme (deux VLC, deux applications Chrome), dont l'une a changé de titre | moyenne | forte | Les deux sources peuvent enregistrer la même fenêtre, et leurs aperçus la montrer | Une fenêtre par source, fenêtre exacte d'une autre jamais prise à défaut ; mêmes règles pour la préparation, les aperçus, les niveaux sonores, la réouverture et « Rétablir la fenêtre » (réalisé le 2026-11-15) | — |
| C7 | Application du Microsoft Store | moyenne | moyenne | Fenêtre attribuée à `ApplicationFrameHost.exe` : son non capturé, source mal reconnue | Processus de la fenêtre `CoreWindow` de l'application (réalisé le 2026-11-15) ; sources ajoutées avant toujours reconnues ; jamais de relance directe (impossible), message à la place | — |

## D - Encodage et disque

| N° | Scénario | Prob. | Grav. | Aujourd'hui | Proposition | Prio. |
|---|---|---|---|---|---|---|
| D1 | Encodeur planté ou figé | faible | forte | Relance automatique dans `…_partie2.mkv`, environ une seconde perdue (testé en simulation) | Rien à ajouter. *2026-11-15 : à l'arrêt, FFmpeg n'est plus tué au bout de 60 s fixes, seulement après 60 s sans progrès (10 min au plus). Encodeur perdu pendant qu'il vide sa file à l'arrêt (ou après une mise en erreur de la source) : une partie de plus vide la file, dans la limite des échecs rapides (testé en simulation)* | — |
| D2 | Trop d'encodages simultanés pour une carte GeForce | faible | forte | NVIDIA limite le nombre d'encodages simultanés **pour tout l'ordinateur**, tous logiciels confondus (OBS, Discord, enregistrement NVIDIA en arrière-plan…) : 12 aujourd'hui selon NVIDIA, au moins 8 avec le pilote 570 exigé par Polyptyque. Au-delà, FFmpeg refuse avec un message trompeur (`OpenEncodeSessionEx failed: out of memory (10)`). L'encodeur refusé échoue trois fois, puis la source passe en erreur avec ce message. La limite réelle sera sans doute plutôt la puissance de la puce (à mesurer, voir le cahier des charges) | Reconnaître ce refus : bascule automatique de la source sur l'encodage par le processeur, avec un message clair (« limite de la carte NVIDIA atteinte, autres logiciels compris ») | P2 |
| D3 | Disque plein | moyenne | forte | Estimation avant le départ, alerte, arrêt propre sous 1 Go libre | Rien à ajouter | — |
| D4 | Disque externe débranché, dossier devenu inaccessible | faible | forte | L'encodeur échoue en boucle, puis chaque source passe en erreur avec un message technique | Bascule automatique vers un dossier de secours (`Vidéos\Polyptyque`) et message clair | P1 |
| D5 | Dossier sur une clé ou un disque en FAT32 | moyenne | moyenne | Fichiers limités à 4 Go par ce format : l'encodeur échoue à 4 Go et repart dans une nouvelle partie (environ une seconde perdue à chaque fois) | Détection à l'enregistrement des réglages et au démarrage : avertissement, et découpage propre avant 4 Go | P2 |
| D6 | Dossier sur un disque lent ou en réseau | moyenne | moyenne | Retard de l'encodeur et images perdues affichés | Avertissement au choix du dossier si le lecteur est amovible ou réseau | P3 |
| D8 | Encodeur qui ne suit plus (machine trop chargée) | moyenne | forte | Les images s'accumulent sans limite (jusqu'à 3 Go dans FFmpeg en 1080p), et la finalisation peut durer très longtemps | File de FFmpeg bornée (environ 128 Mo, 8 à 64 images) ; au-delà de 5 min de retard, source en erreur, fichier fermé proprement (réalisé le 2026-11-15) | — |
| D9 | Dossier protégé par l'« Accès contrôlé aux dossiers » de Windows (Vidéos) | faible | forte | FFmpeg ne peut pas écrire : message technique, bascule inutile sur le processeur | Refus reconnu, message qui en donne la cause (réalisé le 2026-11-15) | — |
| D7 | FFmpeg supprimé ou bloqué par l'antivirus | faible | forte | Au démarrage : message clair. En cours d'enregistrement : la relance échoue et la source passe en erreur | Message dédié (« FFmpeg a disparu : vérifier l'antivirus ») | P3 |

## E - Erreurs de manipulation

| N° | Scénario | Prob. | Grav. | Aujourd'hui | Proposition | Prio. |
|---|---|---|---|---|---|---|
| E1 | Fermer une fenêtre capturée par erreur | forte | forte | Voir C1 | Voir C1 | P1 |
| E2 | Supprimer une source ou une scène par erreur | moyenne | moyenne | Confirmation demandée ; pas d'annulation | Bouton « Annuler » dans le message pendant quelques secondes | P3 |
| E3 | Quitter Polyptyque pendant un enregistrement | moyenne | forte | Confirmation, puis finalisation | Rien à ajouter | — |
| E4 | Lancer Polyptyque deux fois | moyenne | moyenne | Voir A4 | Voir A4 | P1 |
| E5 | Choisir un dossier inadapté (clé, réseau, disque presque plein) | moyenne | moyenne | Estimation de l'autonomie affichée | Voir D5 et D6 | P2 |
| E6 | Débrancher le micro | moyenne | faible | Silence, puis reprise automatique au rebranchement | Rien à ajouter | — |
| E7 | Réglages trop lourds pour la machine (définition, débit, nombre de sources) | moyenne | moyenne | Retard de l'encodeur et images perdues affichés par source | Test de capacité au démarrage (déjà prévu au cahier des charges) | P3 |
| E8 | Oublier un enregistrement en cours | moyenne | moyenne | Arrêt automatique quand le disque est presque plein | Durée maximale facultative dans les réglages | P3 |
| E9 | Déplacer ou renommer un fichier en cours d'enregistrement | faible | forte | **À vérifier** | Selon le test | test |
| E10 | Modifier une scène pendant un enregistrement | moyenne | faible | Impossible (scènes verrouillées) | Rien à ajouter | — |
| E11 | Arrêter par erreur un enregistrement programmé, ou le laisser arrêter un enregistrement manuel | moyenne | moyenne | Arrêt à la main d'un enregistrement programmé confirmé ; un enregistrement manuel n'est arrêté en fin de créneau que si on l'accepte (non par défaut) ; programmation en cours d'enregistrement ni modifiable ni supprimable | Rien à ajouter | — |

---

## Déjà couvert

> [!success] Points solides, vérifiés
> - Endurance : 14 h 15 avec trois sources sans incident, fichiers synchrones, mémoire stable (test réel du 2026-10-21).
> - Encodeur planté ou figé : relance dans une nouvelle partie, image et son recalés (tests de simulation avec un vrai plantage et un vrai blocage de FFmpeg).
> - Fichiers MKV écrits au fil de l'eau, par blocs d'une seconde.
> - Fenêtre perdue : image noire, silence, rattachement automatique (confirmé en test réel).
> - Disque surveillé, arrêt propre ; mise en veille automatique empêchée.
> - Arrêt ou déconnexion de Windows : blocage avec explication, sinon finalisation.
> - Micro débranché, accès au micro refusé : repris ou expliqué.
> - Fermeture de Polyptyque pendant un enregistrement : confirmation et finalisation.
> - Réglages écrits de façon atomique (jamais à moitié).

---

## Plan d'action proposé

### Lot 1 - Reprise après incident

> [!todo] Priorité 1
> - [x] **Journal de session** (A1, B1) : au démarrage d'un enregistrement, Polyptyque note la scène, les sources et les fichiers en cours dans un petit fichier, effacé à l'arrêt normal. S'il est encore là au lancement suivant, c'est qu'il y a eu un incident :
> 	- bilan affiché : quels fichiers, quelle durée, lisibles ou non ;
> 	- **réparation sans réencodage** des fichiers interrompus (index reconstruit, durée correcte), l'original étant gardé jusqu'à la réussite ;
> 	- bouton **« Reprendre l'enregistrement »** (nouveaux fichiers, nommés d'après leur heure de départ comme d'habitude : le suffixe `_reprise` prévu n'a pas été ajouté, le nom suffit à les distinguer).
> - [x] **Fils protégés** (A2) : une erreur interne dans une source est affichée et sa capture relancée.
> - [x] **Instance unique** (A4) : un second lancement ramène la fenêtre existante. *2026-11-15 : aussi face à une copie lancée en administrateur.*
> - [x] **Réglages sauvegardés** (A5) : écriture forcée sur le disque, cinq sauvegardes tournantes, restauration automatique. *2026-11-15 : fichier verrouillé relu cinq fois et jamais écrasé, fichier illisible copié en `.invalide`, fichier d'une version plus récente copié en `config.toml.vN` avec un avertissement.*
> - [x] **Rouvrir et rétablir** (C1, C2, B4) : boutons « Rouvrir » (fenêtre perdue) et « Rétablir la fenêtre » (fenêtre réduite) pendant l'enregistrement ; relance automatique des sources qui l'autorisent.
> - [x] **Dossier de secours** (D4) : si le dossier d'enregistrement devient inaccessible, la suite est écrite dans `Vidéos\Polyptyque`, avec message. *2026-11-15 : un dossier qui ne répond pas en 5 s compte comme inaccessible ; espace libre et tailles lus avec délai, hors du fil de l'interface ; une partie fermée n'est mesurée qu'une fois, celle restée sur le dossier perdu ne fige pas la mesure de la suite.*

### Lot 2 - Situations prévisibles

> [!todo] Priorité 2
> - [x] **Mise en veille** (B3) : *réalisé : arrêt propre et reprise proposée au réveil (choix provisoire).* Polyptyque s'inscrit pour être prévenu par Windows quelques secondes avant la veille, et ferme alors les fichiers proprement. Au réveil, il propose la reprise ou reprend seul dans une nouvelle partie, selon la réponse aux [[#Questions ouvertes]]. La durée de la veille n'est plus comblée d'images figées. *2026-11-15 : une veille dont Windows n'a pas prévenu laisse un saut d'horloge ; au-delà de 5 s passées en veille (mesurées par Windows), la partie est fermée et la suite part dans une nouvelle, sans images figées ni silence. Un fil bloqué sans veille ne coupe pas.*
> - [x] **Après un redémarrage de Windows Update** (B2) : *réalisé : conseil dans le README et le LISEZMOI (pas encore dans l'interface), relance demandée à Windows, bilan « Windows s'est éteint… » avec reprise.* conseil sur les heures d'activité (README et interface) ; bilan et reprise au lancement suivant (lot 1) ; relance automatique de Polyptyque par Windows quand l'utilisateur l'a autorisée (à tester).
> - [ ] **Mises à jour de Polyptyque** (A8) : passer à la première version stable de Velopack qui corrige l'échec du remplacement.
> - [x] **Limite d'encodages de la carte NVIDIA** (D2) : bascule automatique sur le processeur pour la source refusée. *2026-11-15 : refus reconnu dans tout le journal de la partie (FFmpeg 7 et 8 écrivent d'autres erreurs après lui), donc dès le premier échec au lieu du troisième.*
> - [ ] **FAT32** (D5) : avertissement et découpage propre avant 4 Go.
> - [x] **Conversion MP4** (A6) : nom temporaire jusqu'à la vérification. *2026-11-15 : restes d'une conversion ou d'une réparation interrompue remis en ordre au lancement (dossier des enregistrements et de secours) et avant chaque conversion, renommages réessayés ; FFmpeg auxiliaires arrêtés avec Polyptyque (A10).*
> - [x] **Programmation** (B10, E11) : anti-veille, lancement au démarrage de Windows, question du démarrage manqué, confirmations (réalisé le 2026-10-23, à tester sur la machine de référence).

### Lot 3 - Confort et prévention

> [!todo] Priorité 3
> - [x] Journaux supprimés après 30 jours (A7).
> - [ ] Arrêt d'urgence par raccourci, même si l'interface est figée (A3).
> - [ ] Profil de navigateur disparu signalé (C3) ; navigateur déplacé retrouvé (C4).
> - [ ] Annulation d'une suppression (E2).
> - [ ] Avertissement pour un dossier sur disque amovible ou réseau (D6).
> - [x] Message dédié si FFmpeg disparaît (D7).
> - [ ] Test de capacité au démarrage (E7) ; durée maximale facultative (E8).

---

## Tests à mener sur la machine de référence

> [!warning] Sans enjeu
> À faire sur un enregistrement de test de quelques minutes, jamais sur un enregistrement important.

| Test | Comment | Ce qu'on vérifie |
|---|---|---|
| Fermeture brutale | Gestionnaire des tâches > Polyptyque > Fin de tâche, pendant un enregistrement | Les MKV sont lisibles jusqu'à la fermeture (A1, A3) |
| Pilote graphique | <kbd>Win</kbd> + <kbd>Ctrl</kbd> + <kbd>Maj</kbd> + <kbd>B</kbd> (redémarre le pilote graphique, écran noir une seconde) | Capture rattachée, nouvelle partie si l'encodeur a planté (B6) |
| Sortie audio | Brancher ou changer de casque pendant l'enregistrement | Le son des sources continue (B7) |
| Verrouillage | <kbd>Win</kbd> + <kbd>L</kbd> pendant deux minutes, puis déverrouiller | Image pendant le verrouillage, reprise après (B5) |
| Mise en veille | Démarrer > Marche/Arrêt > Mettre en veille pendant un enregistrement, réveiller après deux minutes | Fichiers lisibles, durée du rattrapage d'images figées, image et son repris après le réveil (B3) |
| Renommage | Renommer un MKV en cours d'enregistrement dans l'Explorateur | Refus de Windows ou effet sur l'enregistrement (E9) |
| Mémoire | Relever la mémoire de Polyptyque et des FFmpeg au début et à la fin du test de 12 h | Pas de croissance continue (A9) |
| Programmation verrouillée | Verrouiller la session avant le début d'un créneau, puis éteindre l'écran avec son bouton ; déverrouiller après quelques minutes | Enregistrement démarré à l'heure, image présente dans les fichiers (B11) |

---

## Questions ouvertes

- [ ] **Reprise après incident** : proposée d'un clic, ou automatique (Polyptyque relancé et l'enregistrement repris sans intervention, utile pour une session de nuit sans surveillance) ? *Réalisé provisoirement : d'un clic.*
- [ ] **Mise en veille demandée pendant un enregistrement** : arrêter proprement, ou mettre en pause et reprendre au réveil ?
- [ ] **Réparation des fichiers interrompus** : remplacer le fichier abîmé par sa version réparée, ou garder les deux ? *Réalisé provisoirement : remplacé, une fois la copie vérifiée (même contenu, avec durée et index).*
- [ ] **Durée de conservation des journaux** : 30 jours ?

---

## Sources

> [!info] Consultées le 2026-10-21
> Sources officielles en priorité ; les sources tierces sont signalées.

**Windows**
- [Horodatages haute précision](https://learn.microsoft.com/en-us/windows/win32/sysinfo/acquiring-high-resolution-time-stamps) (Microsoft) : l'horloge QPC continue d'avancer pendant la veille et l'hibernation (B3).
- [std::time::Instant](https://doc.rust-lang.org/std/time/struct.Instant.html) (Rust) : repose sur QPC sous Windows (B3).
- [SetThreadExecutionState](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate) et [PowerSetRequest](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-powersetrequest) (Microsoft) : une mise en veille demandée par l'utilisateur ne peut pas être empêchée (B3).
- [PBT_APMSUSPEND](https://learn.microsoft.com/en-us/windows/win32/power/pbt-apmsuspend) et [WM_POWERBROADCAST](https://learn.microsoft.com/en-us/windows/win32/power/wm-powerbroadcast) (Microsoft) : avertissement environ deux secondes avant la veille, messages de réveil (B3).
- [Desktop Activity Moderator](https://learn.microsoft.com/en-us/windows/win32/w8cookbook/desktop-activity-moderator) et [Préparer un logiciel à la veille moderne](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/prepare-software-for-modern-standby) (Microsoft) : en veille moderne, seules les applications inscrites sont prévenues (B3).
- [RegisterApplicationRestart](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-registerapplicationrestart) et [Relance des applications](https://learn.microsoft.com/en-us/windows/win32/recovery/registering-for-application-restart) (Microsoft) : relance après un plantage ou un redémarrage, conditions (B2).
- [Windows 10 Insider Preview build 18965](https://blogs.windows.com/windows-insider/2019/08/21/announcing-windows-10-insider-preview-build-18965/) (Microsoft) : réglage de relance des applications à la connexion, désactivé par défaut (B2).
- [Redémarrages après une mise à jour](https://learn.microsoft.com/en-us/windows/deployment/update/waas-restart), [Heures d'activité](https://support.microsoft.com/en-us/windows/deployment/updates-lifecycle/keep-your-pc-up-to-date-with-active-hours) et [Échéances imposées](https://learn.microsoft.com/en-us/windows/deployment/update/wufb-compliancedeadlines) (Microsoft) : pas de redémarrage automatique pendant les heures d'activité, plage de 18 h au plus, exceptions (B2).
- [KB5121772](https://support.microsoft.com/en-us/servicing/os/windows/docs/2026/07/kb5121772-one-restart-a-month-for-windows-updates) (Microsoft) : redémarrage hors des heures d'activité « dans la mesure du possible » (B2).

**NVIDIA et FFmpeg**
- [Matrice de prise en charge NVENC](https://developer.nvidia.com/video-encode-and-decode-gpu-support-matrix-new) (NVIDIA) : 12 encodages simultanés sur les GeForce (D2).
- [Note d'application NVENC, SDK 13.1](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/index.html) (NVIDIA) : limite de 12 par ordinateur, toutes cartes confondues (D2).
- [Forum NVIDIA](https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367) : citations des notes des SDK 12.1 et 12.2, aujourd'hui retirées (5 puis 8 encodages, versions de pilote) (D2).
- [Blog vMix](https://blog.vmix.com/?p=4331) (source tierce) : passage à 12 observé avec le pilote 591.44, sans annonce de NVIDIA (D2).
- [Wiki nvidia-patch](https://github.com/keylase/nvidia-patch/wiki/Verify-NVENC-patch) (source tierce) : message de FFmpeg au-delà de la limite (D2).

**Velopack**
- [Velopack sous Windows](https://docs.velopack.io/packaging/operating-systems/windows) : remplacement du dossier du programme pendant une mise à jour (A8).
- [Code d'application des mises à jour, version 1.2.161](https://github.com/velopack/velopack/blob/1.2.161/src/bins/src/commands/apply_windows_impl.rs) et [correctif du 2026-10-10](https://github.com/velopack/velopack/commit/afa30e56b05d434cf4c770f82cbd7ee7231c4ed7) : copie de secours effacée si le second temps échoue (A8).
