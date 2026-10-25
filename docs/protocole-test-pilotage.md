---
title: Polyptyque - Tests du pilotage
aliases: ["Tests pilotage Polyptyque", "Protocole de test du pilotage par un assistant IA"]
tags: [polyptyque, projet, tests, windows, pilotage, mcp]
created: 2026-10-25
status: todo
type: guide
---

# Polyptyque - Tests du pilotage

> [!abstract] Résumé
> Tests à mener sur la machine de référence pour le pilotage par d'autres programmes : réglage, canal de commande (tube nommé), mode commande `polyptyque-cli.exe`, garde-fous (micro, enregistrement visible en arrière-plan), serveur MCP avec Claude Desktop, et non-régression de l'interface, dont plusieurs actions ont été réorganisées pour servir aussi au canal.
> Tout a été vérifié en mode démonstration sur le Mac (socket Unix) ; restent les parties propres à Windows : le tube et ses autorisations, l'icône au point rouge, la notification, le lancement en arrière-plan, les vrais navigateurs et Claude Desktop.
> Comportement de référence : [[cahier-des-charges#Pilotage par d'autres programmes]] ; commandes : [[control|docs/control.md]].

## Table des matières

1. [[#Récupérer la fabrication]]
2. [[#A - Réglage et canal]]
3. [[#B - Mode commande]]
4. [[#C - Garde-fous]]
5. [[#D - Assistant IA (MCP)]]
6. [[#E - Non-régression de l'interface]]
7. [[#F - À me renvoyer]]

> [!tip] Où lancer les commandes
> PowerShell, dans le dossier d'installation : `cd $env:LOCALAPPDATA\PolyptyqueApp\current` (version portable : son sous-dossier `current`), puis `.\polyptyque-cli.exe …`. Le code de retour de la dernière commande s'affiche avec `$LASTEXITCODE`.

---

## Récupérer la fabrication

1. GitHub > dépôt Polyptyque > onglet **Actions** > la dernière exécution verte de la branche `pilotage`. Fabrication vérifiée : [exécution du 2026-10-25](https://github.com/venaciteam/polyptyque/actions/runs/40242057197), [artefact `Polyptyque-windows`](https://github.com/venaciteam/polyptyque/actions/runs/40242057197/artifacts/12617124672) (commit `738150e`).
2. En bas de la page, artefact **`Polyptyque-windows`** : il contient `Polyptyque-Setup.exe` et `Polyptyque-portable.zip`. La version portable évite de toucher à la version installée.
3. Vérifier que le dossier contient `polyptyque-cli.exe` (et plus `polyptyque-console.exe`).

---

## A - Réglage et canal

- [ ] **A1 - Désactivé par défaut**
	- Faire : Polyptyque ouvert, réglage non touché : `.\polyptyque-cli.exe status`.
	- Attendu : erreur `control_disabled`, code de retour **4**, message en français qui indique le réglage à activer.
- [ ] **A2 - Activer**
	- Faire : Réglages > « Pilotage » > « Autoriser le pilotage par d'autres programmes », Enregistrer. Relancer `status`.
	- Attendu : l'état complet en JSON, code **0**. Pendant la commande, une pastille « Pilotage · polyptyque-cli » apparaît un instant en haut.
- [ ] **A3 - Bouton « Connecter un assistant IA… »**
	- Attendu : visible seulement réglage activé ; ouvre la section « Piloter Polyptyque avec un assistant IA » du README français (anglais si Polyptyque est en anglais). ⚠️ La section n'existe sur GitHub qu'une fois la branche fusionnée dans `main` : avant, la page s'ouvre en haut du README.
- [ ] **A4 - Fermé**
	- Faire : quitter Polyptyque, puis `status`.
	- Attendu : `not_running`, code **3**.
- [ ] **A5 - Lancement en arrière-plan**
	- Faire : `.\polyptyque-cli.exe app launch`.
	- Attendu : `{"launched":true,"running":true}` en quelques secondes ; Polyptyque démarre directement près de l'horloge (pas de fenêtre), sans ouvrir les fenêtres de la scène. Le journal dit « lancé en arrière-plan par le mode commande ». Relancé alors qu'il est ouvert : `"launched":false`.
- [ ] **A6 - Désactiver en cours de route**
	- Faire : dans un second PowerShell, `.\polyptyque-cli.exe events` (il affiche l'état en continu) ; puis désactiver le réglage.
	- Attendu : `events` s'arrête avec `not_running` (connexion coupée), la pastille disparaît ; `status` donne `control_disabled`.
- [ ] **A7 - Autorisations du tube** (facultatif, avec [AccessChk](https://learn.microsoft.com/sysinternals/downloads/accesschk))
	- Faire : `whoami /user` pour le SID, puis `accesschk.exe -v \pipe\polyptyque-<SID>`.
	- Attendu : un seul compte (le vôtre), en accès complet ; ni Everyone, ni Anonymous, ni Administrators.

---

## B - Mode commande

- [ ] **B1 - Aide**
	- Faire : `.\polyptyque-cli.exe help`, puis `.\polyptyque-cli.exe help recording start`.
	- Attendu : texte en français, accents corrects ; liste des commandes et des codes de retour. Passer Polyptyque en English (Réglages > Langue) : l'aide passe en anglais.
- [ ] **B2 - JSON et accents dans PowerShell**
	- Faire : `.\polyptyque-cli.exe scenes list | ConvertFrom-Json`.
	- Attendu : les noms de scènes avec accents s'affichent correctement (le JSON écrit `è` : aucun caractère abîmé, quelle que soit la page de code).
- [ ] **B3 - Navigateur**
	- Faire : `.\polyptyque-cli.exe browsers list`, puis `.\polyptyque-cli.exe sources add_browser --browser Firefox --name "Essai pilotage"`.
	- Attendu : la source apparaît dans la liste et sa carte ; Firefox s'ouvre avec un profil vierge ; message « « Essai pilotage » ajoutée… ».
- [ ] **B4 - Fenêtres ouvertes**
	- Faire : ouvrir deux fenêtres du Bloc-notes et une de Discord, puis `.\polyptyque-cli.exe windows list --pretty`.
	- Attendu : chaque fenêtre avec `id` et `program` ; un **titre** seulement pour les deux fenêtres du Bloc-notes, aucun pour Discord.
- [ ] **B5 - Source application**
	- Faire : `.\polyptyque-cli.exe sources add_application --window <id de Discord> --relaunch`.
	- Attendu : source « Discord » ajoutée à la scène en cours.
- [ ] **B6 - Enregistrer**
	- Faire : `.\polyptyque-cli.exe recording start`, attendre 20 s, `status --pretty`, puis `.\polyptyque-cli.exe recording stop`.
	- Attendu : `start` répond une fois l'enregistrement démarré, avec les fenêtres introuvables éventuelles ; le message affiché dans Polyptyque finit par « Demandé par « polyptyque-cli ». » ; `stop` répond avec la liste des fichiers ; les fichiers sont lisibles.
- [ ] **B7 - Verrous et confirmations**
	- Faire : pendant un enregistrement, `.\polyptyque-cli.exe scenes activate --scene <autre scène>` ; hors enregistrement, `.\polyptyque-cli.exe sources remove --source "Essai pilotage"`.
	- Attendu : `locked` (code **5**) ; puis `confirmation_required` (code **7**) avec la question exacte de la fenêtre ; relancé avec `--confirm`, la source est retirée.
- [ ] **B8 - Programmation**
	- Faire : `.\polyptyque-cli.exe schedules create --scene <scène> --start <dans 2 min> --end <dans 4 min> --date <aujourd'hui>`.
	- Attendu : la pastille de la programmation l'affiche ; à l'heure prévue, l'enregistrement démarre et s'arrête comme une programmation créée dans la fenêtre.

---

## C - Garde-fous

- [ ] **C1 - Micro**
	- Faire : `.\polyptyque-cli.exe settings set --microphone.enabled true`.
	- Attendu : la fenêtre de Polyptyque passe au premier plan (même depuis l'arrière-plan) avec « Activer le micro » et le nom du programme demandeur ; la commande attend. « Annuler » → `declined`, rien ne change. Recommencer et accepter → réponse avec le micro activé, pastille du micro en haut. Recommencer sans répondre → `timeout` au bout de deux minutes, fenêtre de confirmation refermée.
- [ ] **C2 - Enregistrement en arrière-plan**
	- Faire : passer Polyptyque en arrière-plan (fermer la fenêtre avec une programmation prévue, ou A5), puis `.\polyptyque-cli.exe recording start`.
	- Attendu : notification de Windows « Enregistrement en cours » avec le message de démarrage ; l'icône près de l'horloge prend un **point rouge** (la chercher sous « ^ » si besoin) ; l'info-bulle dit « Enregistrement en cours ». Après `recording stop`, l'icône redevient normale.
- [ ] **C3 - Enregistrement programmé en arrière-plan**
	- Attendu : même notification et même point rouge pour un enregistrement lancé par une programmation pendant que Polyptyque est en arrière-plan.
- [ ] **C4 - Pastille « Pilotage »**
	- Faire : laisser `.\polyptyque-cli.exe events` tourner.
	- Attendu : « Pilotage · polyptyque-cli events » en haut tant qu'il tourne ; un clic ouvre les réglages (rien pendant un enregistrement) ; elle disparaît à l'arrêt de `events` (Ctrl+C).
- [ ] **C5 - Journal**
	- Attendu : dans le journal du jour, « pilotage : programme connecté (polyptyque-cli.exe, processus …) », une ligne par commande (« pilotage : recording.start (…) »), les refus avec leur raison ; aucun titre de fenêtre.

---

## D - Assistant IA (MCP)

- [ ] **D1 - Brancher Claude Desktop**
	- Faire : suivre la section « Piloter Polyptyque avec un assistant IA » du README (fichier de configuration de Claude Desktop), redémarrer Claude Desktop.
	- Attendu : Polyptyque apparaît parmi les outils de Claude Desktop (une quarantaine d'outils).
- [ ] **D2 - Conversation**
	- Faire : demander à Claude « Qu'est-ce qui est prêt dans Polyptyque ? », puis « Ajoute une source Firefox nommée Invité 2 à la scène Podcast », puis « Lance l'enregistrement de Podcast ».
	- Attendu : Claude appelle `status`/`scenes_list`, puis les bonnes commandes ; la pastille affiche « Pilotage · Claude » ; le message de démarrage cite « Claude ».
- [ ] **D3 - Confirmation**
	- Faire : « Supprime la scène Archive ».
	- Attendu : Claude pose la question de Polyptyque (texte de la fenêtre) avant de supprimer ; sans accord, rien n'est supprimé.
- [ ] **D4 - Polyptyque fermé**
	- Faire : quitter Polyptyque, puis demander l'état à Claude.
	- Attendu : Claude utilise `app_launch` ; Polyptyque démarre en arrière-plan.

---

## E - Non-régression de l'interface

Les actions suivantes passent maintenant par un code commun à la fenêtre et au canal : vérifier qu'elles se comportent comme avant.

- [ ] **E1** Nouvelle scène, renommer, dupliquer (« … (copie) »), supprimer (avec la confirmation qui cite les sources et programmations).
- [ ] **E2** Ajouter une source navigateur (navigateur détecté, puis « Autre navigateur… » avec un chemin, et Tor Browser refusé avec son explication) ; ajouter une source application ; supprimer une source partagée (la confirmation cite les autres scènes).
- [ ] **E3** Réglages : changer chaque réglage, Enregistrer, rouvrir : tout est gardé (langue et horloge appliquées tout de suite, raccourcis, démarrage de Windows, mises à jour, micro).
- [ ] **E4** Programmations : créer, modifier, activer/désactiver (un chevauchement refusé affiche son message), supprimer.
- [ ] **E5** Lancer à la main pendant un créneau : la question « Arrêter automatiquement… » apparaît toujours (le canal, lui, choisit sans question).
- [ ] **E6** Annuler une conversion MP4 en cours (confirmation inchangée).

---

## F - À me renvoyer

- Les cases non cochées, avec ce qui s'est passé à la place.
- Le journal du jour (Réglages > « Signaler un problème… » > « Préparer le signalement », ou le dossier `journaux`).
- Pour C2 : une capture de la notification et de l'icône au point rouge.
