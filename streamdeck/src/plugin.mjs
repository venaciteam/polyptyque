import streamDeck, { SingletonAction } from '@elgato/streamdeck';
import { PolyptyqueClient } from './client.mjs';
import { command, Confirmation, kinds, language, view } from './model.mjs';
import { dataImage } from './render.mjs';

const client = new PolyptyqueClient();
const keys = new Map();
let globalSettings = {};
let commandRunning = false;
const safe = promise => Promise.resolve(promise).catch(() => streamDeck.logger.warn('Mise à jour Stream Deck impossible.'));
const lang = settings => language(settings.language, streamDeck.info.application.language);
const tFor = settings => key => streamDeck.i18n.t(key, lang(settings));

function redraw(entry) {
  const t = tFor(entry.settings);
  let value = view(entry.kind, { ...entry.settings, language: lang(entry.settings) }, client.snapshot(), t);
  if (entry.confirmation.pending) {
    const p = entry.confirmation.pending, snap = client.snapshot();
    if (Date.now() >= p.expires || p.generation !== snap.generation || p.scene !== snap.state?.scene.id ||
        p.scheduledUntil !== snap.state?.recording.scheduled_until) { entry.confirmation.clear(); entry.question = ''; }
    else value = { title: t('key.confirm'), value: t('key.press'), detail: String(Math.ceil((p.expires - Date.now()) / 1000)), tone: 'warning' };
  }
  if (entry.busy) value = { ...value, value: t('key.working') };
  if (entry.error && entry.error.until > Date.now()) value = { ...value, value: t(`error.${entry.error.reason}`), tone: 'warning' };
  const image = dataImage(value);
  if (image !== entry.image) { entry.image = image; safe(entry.action.setImage(image)); }
}
function sendInspector() {
  const id = streamDeck.ui.action?.id;
  if (!id) return;
  const entry = keys.get(id);
  if (!entry) return;
  const snapshot = client.snapshot();
  // Le panneau n'a besoin ni des chemins d'enregistrements ni des détails système.
  safe(streamDeck.ui.sendToPropertyInspector({ type: 'snapshot', context: id, kind: entry.kind,
    online: Boolean(snapshot.state), reason: snapshot.reason, exe: snapshot.exe,
    catalog: snapshot.catalog, settings: entry.settings, globalSettings,
    error: entry.error?.message || '', question: entry.question || '' }));
}
client.on('change', () => { for (const entry of keys.values()) redraw(entry); sendInspector(); });

class PolyptyqueAction extends SingletonAction {
  constructor(kind) { super(); this.kind = kind; this.manifestId = `city.vena.polyptyque.${kind}`; }
  onWillAppear(ev) {
    const entry = { action: ev.action, kind: this.kind, settings: ev.payload.settings, confirmation: new Confirmation() };
    keys.set(ev.action.id, entry); safe(ev.action.setTitle('')); redraw(entry);
  }
  onWillDisappear(ev) { keys.delete(ev.action.id); }
  onDidReceiveSettings(ev) {
    const entry = keys.get(ev.action.id);
    if (entry) { entry.settings = ev.payload.settings; entry.confirmation.clear(); entry.error = null; entry.question = ''; redraw(entry); sendInspector(); }
  }
  onPropertyInspectorDidAppear() { sendInspector(); }
  async onSendToPlugin(ev) {
    try {
      if (ev.payload?.type === 'ready') sendInspector();
      if (ev.payload?.type === 'connection') {
        const path = ev.payload.cliPath;
        if (typeof path !== 'string' || path.length > 32768) return;
        globalSettings = { cliPath: path.trim() };
        await streamDeck.settings.setGlobalSettings(globalSettings);
        client.start(globalSettings.cliPath); sendInspector();
      }
      if (ev.payload?.type === 'retry') client.start(globalSettings.cliPath || '');
    } catch { streamDeck.logger.warn('Réglages de connexion indisponibles.'); }
  }
  async onKeyDown(ev) {
    const entry = keys.get(ev.action.id);
    if (!entry || commandRunning) return;
    let snapshot = client.snapshot();
    const settings = entry.settings;
    commandRunning = true; entry.busy = true; entry.error = null; entry.question = ''; redraw(entry);
    let request;
    try {
      if (this.kind === 'schedule') {
        // Une seconde pression rapide et une modification dans Polyptyque doivent lire la vraie valeur.
        const schedules = await client.call('schedules.list');
        client.catalog.schedules = schedules.schedules;
        snapshot = client.snapshot();
      }
      request = command(this.kind, settings, snapshot);
      if (!request) return;
      if (entry.confirmation.take(request, settings, snapshot)) request = { ...request, params: { ...request.params, confirm: true } };
      await client.call(request.method, request.params);
      entry.confirmation.clear();
      // L'état des touches sera celui des notifications ; seule la liste des programmations nécessite une relecture.
    } catch (error) {
      if (error.reason === 'confirmation_required' && request && !request.params.confirm && client.state && snapshot.generation === client.generation &&
          JSON.stringify(settings) === JSON.stringify(entry.settings) && snapshot.state?.scene.id === client.state.scene.id &&
          snapshot.state?.recording.scheduled_until === client.state.recording.scheduled_until) {
        entry.confirmation.remember(request, settings, snapshot); entry.question = error.question || error.message;
      } else {
        entry.confirmation.clear();
        entry.error = { reason: error.reason || 'internal', message: error.message, until: Date.now() + 5000 };
        safe(ev.action.showAlert());
      }
    } finally { commandRunning = false; entry.busy = false; redraw(entry); sendInspector(); }
  }
}
for (const kind of kinds) streamDeck.actions.registerAction(new PolyptyqueAction(kind));
streamDeck.settings.onDidReceiveGlobalSettings(ev => {
  const next = ev.settings || {};
  if (next.cliPath !== globalSettings.cliPath) { globalSettings = next; client.start(globalSettings.cliPath || ''); }
});
// Ne pas masquer une erreur fatale : le processus s'arrête, Stream Deck peut le relancer.
process.on('uncaughtException', () => { streamDeck.logger.error('Erreur fatale du plugin.'); client.stop(); process.exit(1); });
process.on('unhandledRejection', () => streamDeck.logger.error('Opération asynchrone du plugin refusée.'));
await streamDeck.connect();
globalSettings = await streamDeck.settings.getGlobalSettings();
client.start(globalSettings.cliPath || '');
setInterval(() => { for (const entry of keys.values()) redraw(entry); }, 500).unref();
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => { client.stop(); process.exit(0); });
