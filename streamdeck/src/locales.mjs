// Français source ; anglais natif. Les deux panneaux utilisent ces mêmes ressources.
export const fr = {
  action: { recording: 'Enregistrement', source: 'Source', scene: 'Scène', windows: 'Fenêtres', previews: 'Aperçus', schedule: 'Programmation', disk: 'Espace disque' },
  tooltip: { recording: 'Démarrer ou arrêter toutes les sources de la scène.', source: 'Démarrer ou arrêter une source de la scène active. Durée de la session et état de la fenêtre.', scene: 'Activer une scène.', windows: 'Ouvrir les fenêtres de la scène ou ouvrir/rétablir une source.', previews: 'Activer ou couper les aperçus.', schedule: 'Activer ou suspendre une programmation.', disk: 'Espace disponible et durée d’enregistrement estimée.' },
  key: { ready: 'Prêt', recording: 'En cours', failed: 'Échec', active: 'Active', on: 'Activés', off: 'Coupés', enabled: 'Active', paused: 'Suspendue', confirm: 'Confirmer', press: 'Appuyer à nouveau', working: 'En cours…' },
  window: { found: 'Fenêtre trouvée', minimized: 'Fenêtre réduite', missing: 'Fenêtre perdue', searching: 'Recherche…', unknown: 'Non vérifiée' },
  day: { mon: 'lun.', tue: 'mar.', wed: 'mer.', thu: 'jeu.', fri: 'ven.', sat: 'sam.', sun: 'dim.' },
  error: { connecting: 'Connexion…', cli_missing: 'CLI introuvable', not_running: 'Polyptyque fermé', control_disabled: 'Pilotage désactivé', configure: 'À configurer', source_inactive: 'Source absente', target_missing: 'Cible absente', busy: 'Occupé', locked: 'Verrouillé', refused: 'Refusé', declined: 'Refusé', timeout: 'Délai dépassé', unavailable: 'Indisponible', connection_changed: 'Connexion coupée', invalid_response: 'Réponse invalide', not_found: 'Introuvable', ambiguous: 'Cible ambiguë', invalid_request: 'Réglage invalide', internal: 'Erreur', confirmation_required: 'Confirmation requise' },
  ui: {
    connection: 'Connexion à Polyptyque', connected: 'Connecté à Polyptyque', waiting: 'En attente du plugin…', panelDisconnected: 'Panneau déconnecté',
    noPlugin: 'Le plugin ne répond pas. Fermer ce panneau puis le rouvrir ; consulter le journal Stream Deck si nécessaire.',
    setup: 'Connexion', instructions: '1. Ouvrir Polyptyque : le plugin le détecte automatiquement.\n2. Dans Réglages, activer « Autoriser le pilotage par d’autres programmes ».\n3. Les scènes et sources apparaissent ci-dessous.',
    manual: 'Chemin manuel (facultatif)', autoDetected: 'Polyptyque détecté automatiquement.', manualConfigured: 'Chemin personnalisé utilisé.',
    path: 'Chemin de polyptyque-cli.exe', pathHint: 'Uniquement pour une copie portable non détectée. Laisser vide pour la détection automatique.',
    apply: 'Appliquer à toutes les touches', retry: 'Réessayer la connexion', language: 'Langue', auto: 'Automatique (Stream Deck)',
    mode: 'À la pression', toggle: 'Basculer', on: 'Activer / démarrer', off: 'Suspendre / arrêter',
    scene: 'Scène', source: 'Source', schedule: 'Programmation', scope: 'Fenêtres à ouvrir', all: 'Toute la scène active', one: 'Une source de la scène active', choose: 'Choisir…', missing: 'Cible enregistrée introuvable',
    confirm: 'Arrêt programmé : appuyer une seconde fois sur la touche dans les 5 secondes pour confirmer.',
    sourceNote: 'La source doit appartenir à la scène active. La durée affichée est celle de la session, y compris si cette source la rejoint plus tard.',
    diskNote: 'Estimation selon le débit et les sources de la scène. La durée réelle dépend du contenu enregistré.',
    scheduleNote: 'Une programmation par touche. Suspendre une programmation n’arrête pas l’enregistrement déjà lancé.',
    error: 'Dernière erreur', refresh: 'Les listes se mettent à jour automatiquement.',
    cli_missing: 'Ouvrir Polyptyque pour permettre sa détection automatique. Le chemin manuel est un recours pour les copies portables non détectées.',
    not_running: 'Ouvrir Polyptyque, puis réessayer.', control_disabled: 'Dans les réglages de Polyptyque, activer « Autoriser le pilotage par d’autres programmes ».',
    version: 'Plugin bêta 0.1.0.1 · Polyptyque 1.0.0-beta.9 ou suivant'
  }
};
export const en = {
  action: { recording: 'Recording', source: 'Source', scene: 'Scene', windows: 'Windows', previews: 'Previews', schedule: 'Scheduled recording', disk: 'Disk space' },
  tooltip: { recording: 'Start or stop all sources in the current scene.', source: 'Start or stop a source in the active scene. Shows session duration and window status.', scene: 'Switch to a scene.', windows: 'Open the scene’s windows, or open/restore one source.', previews: 'Turn previews on or off.', schedule: 'Enable or pause a scheduled recording.', disk: 'Available space and estimated recording time.' },
  key: { ready: 'Ready', recording: 'Recording', failed: 'Failed', active: 'Active', on: 'On', off: 'Off', enabled: 'Enabled', paused: 'Paused', confirm: 'Confirm', press: 'Press again', working: 'Working…' },
  window: { found: 'Window found', minimized: 'Minimized', missing: 'Window lost', searching: 'Searching…', unknown: 'Not checked' },
  day: { mon: 'Mon', tue: 'Tue', wed: 'Wed', thu: 'Thu', fri: 'Fri', sat: 'Sat', sun: 'Sun' },
  error: { connecting: 'Connecting…', cli_missing: 'CLI not found', not_running: 'Polyptyque closed', control_disabled: 'Control disabled', configure: 'Set up', source_inactive: 'Source absent', target_missing: 'Target absent', busy: 'Busy', locked: 'Locked', refused: 'Refused', declined: 'Declined', timeout: 'Timed out', unavailable: 'Unavailable', connection_changed: 'Disconnected', invalid_response: 'Invalid response', not_found: 'Not found', ambiguous: 'Ambiguous target', invalid_request: 'Invalid setting', internal: 'Error', confirmation_required: 'Confirm first' },
  ui: {
    connection: 'Connect to Polyptyque', connected: 'Connected to Polyptyque', waiting: 'Waiting for the plugin…', panelDisconnected: 'Panel disconnected',
    noPlugin: 'The plugin isn’t responding. Close and reopen this panel; check the Stream Deck log if needed.',
    setup: 'Connection', instructions: '1. Open Polyptyque: the plugin detects it automatically.\n2. In Settings, enable “Allow control by other programs”.\n3. Scenes and sources appear below.',
    manual: 'Manual path (optional)', autoDetected: 'Polyptyque detected automatically.', manualConfigured: 'Using a custom path.',
    path: 'Path to polyptyque-cli.exe', pathHint: 'Only for an undetected portable copy. Leave blank for automatic detection.',
    apply: 'Apply to all keys', retry: 'Retry connection', language: 'Language', auto: 'Automatic (Stream Deck)',
    mode: 'On press', toggle: 'Toggle', on: 'Enable / start', off: 'Pause / stop',
    scene: 'Scene', source: 'Source', schedule: 'Scheduled recording', scope: 'Windows to open', all: 'All windows in the active scene', one: 'One source in the active scene', choose: 'Choose…', missing: 'Saved target not found',
    confirm: 'To stop a scheduled recording, press the key again within 5 seconds to confirm.',
    sourceNote: 'The source must be in the active scene. The duration shown is the session’s, even if this source joined later.',
    diskNote: 'Estimated from the bitrate and the scene’s sources. Actual recording time depends on the content.',
    scheduleNote: 'One scheduled recording per key. Pausing a schedule doesn’t stop a recording that has already started.',
    error: 'Last error', refresh: 'Lists update automatically.',
    cli_missing: 'Open Polyptyque so it can be detected automatically. A manual path is a fallback for undetected portable copies.',
    not_running: 'Open Polyptyque and retry.', control_disabled: 'In Polyptyque’s settings, enable “Allow control by other programs”.',
    version: 'Beta plugin 0.1.0.1 · Polyptyque 1.0.0-beta.9 or later'
  }
};
export function translate(lang, key) {
  return key.split('.').reduce((o, k) => o?.[k], lang === 'fr' ? fr : en) ?? key.split('.').reduce((o, k) => o?.[k], en) ?? key;
}
