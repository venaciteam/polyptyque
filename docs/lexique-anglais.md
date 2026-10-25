---
title: Polyptyque - Lexique anglais
aliases: ["Lexique anglais Polyptyque", "English glossary"]
tags: [polyptyque, traduction, anglais, lexique]
created: 2026-10-24
status: done
type: doc
---

# Polyptyque - Lexique anglais

> [!abstract] Résumé
> Termes de référence et règles d'écriture de la version anglaise de Polyptyque, fixés **avant** de traduire, pour que les 516 textes du catalogue (`crates/polyptyque-app/lang/en/LC_MESSAGES/polyptyque-app.po`) parlent tous la même langue. La version anglaise est écrite directement en anglais, pas calquée sur le français (voir [[cahier-des-charges#Langues et formats]]).

## Table des matières

1. [[#Décisions]]
2. [[#Principes d'écriture]]
	1. [[#Ton]]
	2. [[#Casse et typographie]]
	3. [[#Nombres, unités, dates]]
3. [[#Lexique de référence]]
	1. [[#Concepts]]
	2. [[#Enregistrement et sources]]
	3. [[#Programmation]]
	4. [[#Réglages et encodage]]
	5. [[#Fichiers, conversion, réparation]]
	6. [[#Mises à jour et signalement]]
4. [[#Jamais traduits]]
5. [[#Règles pour le catalogue]]

---

## Décisions

> [!success] Tranché par Leeva le 2026-10-24

| # | Question | Choix | Pourquoi |
|---|---|---|---|
| 1 | Programmation / créneau | **scheduled recording** / **time slot** | « Recording window » écarté pour le créneau : dans un logiciel qui enregistre des fenêtres, « window » serait ambigu |
| 2 | Micro | **Mic** (pastille, libellés, nom de fichier `…_Mic.mka`) | Court et courant ; « microphone » reste possible dans une phrase qui parle du périphérique Windows |
| 3 | Source application, bouton « Application » | **app source**, bouton **App** | Windows 11 parle d'« apps » partout |
| 4 | « Supprimer la source » | **Remove source** ; la scène : **Delete scene** | Une source partagée n'est retirée que de la scène en cours |
| 5 | Formes contractées (*can't*, *doesn't*) | **Oui** | Ton naturel, recommandé par le guide de style de Microsoft |
| 6 | Guillemets et apostrophe | **“ ”** et **’** typographiques | Même soin que les « » du français |
| 7 | Orthographe | **Américaine** (*canceled*, *color*) | Norme du logiciel, cohérente avec Windows en anglais |

---

## Principes d'écriture

### Ton

- Direct et sobre : l'information d'abord, sans formule de politesse vide. Pas de *please* de routine, pas d'*Oops*, pas de point d'exclamation.
- *You* quand il faut s'adresser à la personne ; impératif pour une consigne (*Add a source to this scene first.*).
- **Jamais *we*** : l'émetteur n'est pas une équipe. Le sujet est Polyptyque (*Polyptyque couldn't open the browser.*), comme en français.
- Pas de genre : *they* si une personne est évoquée.
- Phrases complètes ; un message d'erreur dit ce qui s'est passé, puis quoi faire.

### Casse et typographie

- **Casse de phrase partout** (titres, boutons, menus) : *Start recording*, *Report a problem…*, pas *Start Recording*. Exception : les titres des panneaux restent en capitales, comme en français (*SCENES*, *SOURCES*, *CONTROLS*).
- **Points de suspension** sur un bouton qui ouvre une fenêtre ou un choix (*Browse…*, *Convert to MP4…*), comme en français.
- Pas d'espace avant `:` `?` `!` ; deux-points suivis d'une minuscule, sauf nom propre.
- Noms entre guillemets typographiques : *“Discord” added.*
- Un texte français qui finit par un point le garde en anglais, et inversement.

### Nombres, unités, dates

| Élément | Anglais | Français |
|---|---|---|
| Décimales, milliers | `1.2 GB`, `12,000 kbps` | `1,2 Go`, `12 000 kb/s` |
| Tailles | GB, MB, KB | Go, Mo, Ko |
| Débit | kbps | kb/s |
| Cadence | fps | i/s |
| Durées | `31 h`, `45 min`, `12 s` | idem |
| Heures, dates | voir [[cahier-des-charges#Langues et formats]] | idem |
| Noms de fichiers | `2026-10-20_14-32_Podcast_Discord.mkv` | `2026-10-20_14h32_Podcast_Discord.mkv` |

---

## Lexique de référence

### Concepts

| Français | Anglais | Note |
|---|---|---|
| scène | scene | |
| source | source | |
| source navigateur | browser source | Un vrai navigateur avec son propre profil (rien à voir avec la page intégrée d'autres logiciels) |
| source application | app source | Bouton d'ajout : *App* |
| profil (de navigateur) | (browser) profile | |
| partagée (entre scènes) | shared | *Shared with 2 scenes* |
| fenêtre | window | |
| aperçu en direct | live preview | |
| réglages | Settings | Titre de la fenêtre : *Settings* |
| journal | log | *(details in the log)* |

### Enregistrement et sources

| Français | Anglais |
|---|---|
| Démarrer l'enregistrement | Start recording |
| Tout arrêter | Stop all |
| Enregistrer cette source / Arrêter cette source | Record this source / Stop this source |
| ENREGISTREMENT 00:12:34 | RECORDING 00:12:34 |
| Prêt | Ready |
| Enregistre | Recording |
| Préparation… / Finalisation… | Preparing… / Finalizing… |
| Fichier terminé / Terminée | File finished / Finished |
| Son actif / Muet | Sound / Silent |
| {n} perdue(s) (images) | {n} dropped |
| Fenêtre introuvable / perdue / réduite | Window not found / lost / minimized |
| Rétablir la fenêtre / Rouvrir la fenêtre | Restore window / Reopen window |
| Ouvrir les fenêtres | Open windows |
| Ajouter une source navigateur / application | Add a browser source / an app source |
| Monter / Descendre la source | Move source up / down |
| Nouvelle scène, Renommer, Dupliquer, Supprimer la scène | New scene, Rename, Duplicate, Delete scene |
| Copier / Coller | Copy / Paste |
| Enregistrement interrompu | Recording interrupted |
| Reprendre l'enregistrement | Resume recording |

### Programmation

| Français | Anglais |
|---|---|
| programmation(s) | scheduled recording(s) |
| enregistrement programmé | scheduled recording |
| Programmer (pastille) | Schedule |
| Programmations (titre de la fenêtre) | Scheduled recordings |
| Nouvelle programmation / Modifier la programmation | New scheduled recording / Edit scheduled recording |
| Activer / Désactiver la programmation | Turn on / Turn off this scheduled recording |
| créneau | time slot |
| Créneau en cours | Time slot in progress |
| Arrêt programmé à 22:00 | Scheduled stop at 10:00 PM (horloge selon Windows) |
| Démarrage manqué | Missed start |
| Une seule fois / Tous les jours / Certains jours de la semaine | Once / Every day / Specific days of the week |
| Du lundi au vendredi / Le week-end / Chaque lundi | Weekdays / Weekends / Every Monday |
| aujourd'hui à… / demain à… | today at… / tomorrow at… |
| arrière-plan (Continuer en arrière-plan) | background (Keep running in the background) |
| icône près de l'horloge (zone de notification) | icon next to the clock (*notification area* : terme évité, Windows 11 dit aussi *system tray*) |
| Afficher Polyptyque / Quitter Polyptyque (menu de l'icône) | Show Polyptyque / Quit Polyptyque |
| Se souvenir de mon choix | Remember my choice |
| Demander à chaque fois | Ask every time |
| Se termine le lendemain. | Ends the next day. |
| Lancer Polyptyque au démarrage de Windows | Start Polyptyque when Windows starts |

### Réglages et encodage

| Français | Anglais |
|---|---|
| Enregistrement / Fichiers et interface (colonnes) | Recording / Files and interface |
| Résolution | Resolution |
| Images par seconde | Frame rate |
| Débit vidéo (kb/s) / Débit audio | Video bitrate (kbps) / Audio bitrate |
| Format vidéo | Video format |
| Matériel d'encodage | Encoding hardware |
| Automatique (carte graphique si disponible) / Carte graphique / Processeur | Automatic (graphics card if available) / Graphics card / CPU |
| encodeur | encoder |
| Dossier des enregistrements | Recordings folder |
| Modèle de nom de fichier | File name pattern |
| Variables : {date}, {heure}, {scène}, {source} | Variables: {date}, {time}, {scene}, {source} |
| Raccourci : … | Shortcut: … |
| Ctrl+Maj+F9 | Ctrl+Shift+F9 |
| Aucun (raccourci) | None |
| Micro par défaut de Windows | Windows default mic |
| Enregistrer / Annuler (réglages) | Save / Cancel |
| (personnalisé) / (débranché) | (custom) / (unplugged) |

### Fichiers, conversion, réparation

| Français | Anglais |
|---|---|
| Fichiers (bouton) | Files |
| Convertir en MP4… | Convert to MP4… |
| Conversion pour le montage | Conversion for editing |
| Réparer / Réparer les fichiers | Repair / Repair files |
| partie 2 (suite d'un fichier) | part 2 ; nom de fichier `…_part2.mkv` |
| Réparé : prêt pour le montage | Repaired: ready for editing |

### Mises à jour et signalement

| Français | Anglais |
|---|---|
| Mises à jour / Versions stables / Versions stables et bêta / Ne pas rechercher | Updates / Stable versions / Stable and beta versions / Don't check |
| Mettre à jour | Update |
| Redémarrer pour passer à la version {} | Restart to update to {} |
| Signaler un problème… / Signaler ce problème | Report a problem… / Report this problem |
| Préparer le signalement | Prepare the report |
| rapport (fichier) | report ; fichier `report_….txt` |
| (détail dans le journal) | (details in the log) |

---

## Jamais traduits

Polyptyque, Windows, GitHub, FFmpeg, MKV, MKA, MP4, M4A, H.264, HEVC, NVIDIA NVENC, AMD AMF, Intel Quick Sync, Tor Browser, noms des navigateurs.

---

## Règles pour le catalogue

- Un même texte français a **une seule** traduction : s'il doit en avoir deux (homonyme), lui donner un contexte dans le code (`@tr("contexte" => "texte")`, `tr!("contexte" => "texte")`). À surveiller : « Terminée » (source finie, programmation sans autre créneau), « Enregistrer » (seulement *Save* aujourd'hui).
- Mêmes paramètres `{}` que le français, dans l'ordre voulu par la phrase anglaise (`{0}`, `{1}` si l'ordre change). Le test du catalogue vérifie les paramètres.
- Pluriels : deux formes, singulier pour 1 (*1 file*, *2 files*, *0 files*).
- Textes à garder en anglais tels que le formulaire GitHub les cite : *Settings › Report a problem…*.
