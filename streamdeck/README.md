# Polyptyque pour Stream Deck · bêta

[English](README.en.md)

Plugin Windows en français et en anglais, avec l'identité visuelle de Polyptyque. Compatible avec **Polyptyque 1.0.0-beta.9 ou suivant** et **Stream Deck 7.1 ou suivant**. Aucun compte supplémentaire, aucune clé API, aucune connexion Internet pour fonctionner.

## Installer et connecter

Le paquet **0.1.0.2** corrige l'enregistrement des choix (scène, source, programmation) et le dialogue du panneau. Chaque changement est confirmé par relecture des réglages de Stream Deck ; un ancien état ne fait plus revenir la sélection en arrière. Ouvrir le nouveau paquet pour mettre à jour l'installation ; les identifiants des actions restent identiques.

1. [Télécharger le plugin bêta 0.1.0.2](https://github.com/venaciteam/polyptyque/releases/download/v1.0.0-beta.9/city.vena.polyptyque.streamDeckPlugin), ouvrir le fichier et accepter son installation dans Stream Deck.
2. Ouvrir Polyptyque. Dans **Réglages**, activer **Autoriser le pilotage par d'autres programmes**.
3. Dans Stream Deck, trouver la catégorie **Polyptyque**, puis glisser une action sur une touche. Le panneau affiche **Connecté à Polyptyque** quand l'état réel est reçu.
4. Pour une action Source, Scène ou Programmation, choisir sa cible dans la liste. Pour Aperçus ou Enregistrement, choisir **Basculer**, **Activer / démarrer** ou **Suspendre / arrêter**.

Le message **Réglage enregistré dans Stream Deck** confirme le choix. Pour une scène, cette étape configure la touche ; appuyer ensuite sur la touche pour activer la scène dans Polyptyque.

Le plugin détecte d'abord le Polyptyque ouvert (portable compris), puis son installation standard. Le panneau confirme **Polyptyque détecté automatiquement** ; aucun chemin n'est demandé pour une copie détectée. Avec une version portable non détectée, déplier **Connexion › Chemin manuel (facultatif)**, renseigner le chemin complet de `polyptyque-cli.exe` livré avec elle (dans `current`), puis cliquer sur **Appliquer à toutes les touches**. Laisser ce champ vide pour la détection automatique. Aucun fichier de configuration à modifier. La langue suit Stream Deck ; chaque touche peut imposer français ou anglais.

## Actions et retours

| Action | À la pression | État affiché |
|---|---|---|
| Enregistrement | Démarrer/arrêter toute la scène active | Durée de la session, rouge pendant l'enregistrement |
| Source | Démarrer/arrêter une source de la scène active | Durée de la session, fenêtre perdue/réduite signalée en jaune |
| Scène | Activer la scène choisie | Nom, vert si active |
| Fenêtres | Ouvrir les fenêtres de la scène active, ou ouvrir/rétablir une source | Fenêtres trouvées/total, ou état de la source |
| Aperçus | Activer/couper | État réel, vert si actifs |
| Programmation | Activer/suspendre la programmation choisie | Scène, horaire, état réel |
| Espace disque | Aucune commande | Espace libre, durée estimée ; jaune sous une heure |

La durée d'une touche Source est celle de la **session**, même si la source la rejoint plus tard. Une programmation par touche ; suspendre une programmation n'arrête pas un enregistrement déjà lancé et peut être refusé si Polyptyque la verrouille. Un bouton « démarrer » pendant un enregistrement en cours ne relance rien ; une source supplémentaire se lance avec sa propre touche.

Pour arrêter un enregistrement pris en charge par la programmation, la première pression affiche **Confirmer / Appuyer à nouveau**. Appuyer une seconde fois dans les **5 secondes** ; la question complète apparaît aussi dans le panneau. Une coupure, un changement de scène ou un changement de réglage annule cette confirmation. Les verrouillages et l'accord du micro restent gérés dans Polyptyque.

Si Polyptyque se ferme ou si le pilotage est désactivé, les touches passent hors ligne. Les commandes interrompues ne sont jamais rejouées automatiquement : vérifier l'état après reconnexion avant de presser à nouveau. Les scènes, sources et programmations sont relues toutes les quatre secondes ; les états d'enregistrement et de fenêtres proviennent des notifications locales. Une cible supprimée reste signalée, sans remplacement silencieux.

Les raccourcis globaux restent utilisables avec l'action Stream Deck **Raccourci clavier**, sans retour d'état : Ctrl+Maj+F9 (enregistrement), Ctrl+Maj+F10 (fenêtre Polyptyque), Ctrl+Maj+F11 (aperçus), sauf personnalisation dans Polyptyque.

## Construire et vérifier

Depuis ce dossier, avec Node.js 24 ou suivant :

```powershell
npm ci
npm test
npm run test:ui
npm run pack
```

Les itérations et les essais utilisent le **build local**, sans attendre GitHub. Le workflow GitHub ne démarre que sur un tag de publication `v*` ou sur demande manuelle ; les push de branche et les PR ne déclenchent pas de fabrication.

`test:ui` utilise Chrome en mode headless, installé sur la machine. Le workflow de publication utilise Chromium installé par Playwright (`POLYPTYQUE_TEST_BROWSER=chromium`) et exécute aussi ces tests. L'hôte de test authentifie les sorties avec l'UUID du panneau, puis répond avec le contexte de la touche, comme l'hôte natif. Les tests vérifient la persistance des trois cibles après réouverture, les notifications périmées et la perte/reprise du dialogue. Le paquet et les aperçus sont écrits dans `../livraison/` ; le dossier `.sdPlugin` est généré et ignoré par Git. Installer le **paquet**, pas un lien symbolique vers ce dossier. À chaque nouveau paquet destiné à être installé, incrémenter la version à quatre composantes du manifeste dans `tools/build.mjs` (une version identique peut être ignorée par Stream Deck). La version du plugin est indépendante de celle de Polyptyque.

Le manifeste et les fichiers de traduction sont générés depuis `src/locales.mjs`. Les tests vérifient la résolution des traductions par le fournisseur SDK réel, les transitions des actions, les confirmations, les erreurs CLI, les coupures et le dialogue du paquet avec un hôte Stream Deck simulé. Les tests du panneau couvrent les deux langues, la cible supprimée, le pilotage désactivé et le changement de chemin sans rechargement. Il reste à valider les pressions sur un appareil physique et les retours auprès d'une vraie session de capture.

Architecture : notifications `polyptyque-cli events`, commandes `polyptyque-cli call`, processus sans shell et fenêtre console masquée. Le CLI vérifie le compte propriétaire du tube Windows. Le plugin n'écrit pas dans les fichiers de Polyptyque, n'ouvre pas de serveur HTTP et ne contourne aucune confirmation. Le seul WebSocket est celui que Stream Deck impose à ses plugins et à leur panneau.

Les ressources distribuées restent immuables ; les réglages sont conservés par Stream Deck. Les journaux du SDK utilisent son répertoire standard. Les noms des scènes/sources et les paramètres des commandes ne sont pas journalisés par le plugin. Les dépendances sont verrouillées ; `zod` est remplacé par la version corrigée 4.6.6 ou suivante, compatible avec l'import SDK `zod/v4-mini`.

## Diffusion

Le paquet bêta 0.1.0.2 est disponible en téléchargement direct dans la préversion GitHub de Polyptyque 1.0.0-beta.9, avec laquelle il est compatible. La candidature au **Marketplace Elgato** reste à décider ; aucune soumission n'est effectuée par la construction du paquet. Licence GPL-3.0-or-later, comme Polyptyque ; licences des dépendances incluses.
