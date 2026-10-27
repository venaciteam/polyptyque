import { translate } from './locales.mjs';
import { language } from './model.mjs';
import { InspectorSettings } from './inspector-settings.mjs';

let socket, inspectorContext, actionContext, action, kind, systemLanguage = 'fr', settings = {}, globalSettings = {}, snapshot;
let savedSettings = new InspectorSettings();
let lastReply = 0, initialized = false, pathDirty = false, panelDisconnected = false, startedAt = Date.now();
const $ = id => document.getElementById(id);
const t = key => translate(language(settings.language, systemLanguage), key);
function send(event, payload) {
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ event, context: inspectorContext, action, payload }));
}
const plugin = payload => send('sendToPlugin', payload);
function localize() {
  document.documentElement.lang = language(settings.language, systemLanguage);
  for (const node of document.querySelectorAll('[data-i18n]')) node.textContent = t(node.dataset.i18n);
}
function renderConnection() {
  $('connection').classList.toggle('online', !panelDisconnected && Boolean(snapshot?.online));
  $('status').textContent = panelDisconnected ? t('ui.panelDisconnected') : snapshot?.online ? t('ui.connected') : snapshot ? t(`error.${snapshot.reason || 'not_running'}`) : t('ui.waiting');
  const helpKey = snapshot?.reason && `ui.${snapshot.reason}`;
  $('help').textContent = panelDisconnected ? t('ui.noPlugin') : helpKey && t(helpKey) !== helpKey ? t(helpKey) : '';
}
function receiveSettings(value) { savedSettings.receive(value); settings = savedSettings.current; }
function renderSaveStatus() {
  const pending = savedSettings.pending;
  $('settings-status').hidden = !savedSettings.edited;
  $('settings-status').textContent = !pending ? t('ui.saved') : Date.now() - pending.since > 5000 ? t('ui.saveFailed') : t('ui.saving');
}
function targets(id, values, selected, label) {
  const select = $(id), signature = JSON.stringify([values, selected, language(settings.language, systemLanguage)]);
  if (select.dataset.signature === signature) return;
  select.dataset.signature = signature;
  const options = [new Option(t('ui.choose'), '')];
  for (const value of values || []) options.push(new Option(label(value), value.id));
  if (selected && !(values || []).some(v => v.id === selected)) options.push(new Option(`${t('ui.missing')} (${selected})`, selected));
  select.replaceChildren(...options); select.value = selected || '';
}
function render() {
  localize();
  $('mode-field').hidden = !['recording', 'source', 'previews', 'schedule'].includes(kind);
  $('scope-field').hidden = kind !== 'windows'; $('scene-field').hidden = kind !== 'scene';
  $('source-field').hidden = kind !== 'source' && !(kind === 'windows' && settings.scope === 'source');
  $('schedule-field').hidden = kind !== 'schedule'; $('disk-note').hidden = kind !== 'disk';
  $('confirm-note').hidden = !['recording', 'source'].includes(kind);
  $('language').value = settings.language || 'auto'; $('mode').value = settings.mode || 'toggle'; $('scope').value = settings.scope || 'scene';
  const catalog = snapshot?.catalog || { scenes: [], sources: [], schedules: [] };
  targets('scene', catalog.scenes, settings.scene, v => v.name);
  targets('source', catalog.sources, settings.source, v => v.name);
  targets('schedule', catalog.schedules, settings.schedule, v => {
    const date = v.date ? new Intl.DateTimeFormat(language(settings.language, systemLanguage), { dateStyle: 'medium' }).format(new Date(`${v.date}T12:00:00`)) :
      (v.days || []).map(day => t(`day.${day}`)).join(', ');
    return `${v.scene.name} · ${v.start}–${v.end} · ${date}`;
  });
  renderConnection();
  renderSaveStatus();
  if (!pathDirty) $('cli').value = globalSettings.cliPath || '';
  $('detected').textContent = snapshot?.exe ? t(globalSettings.cliPath ? 'ui.manualConfigured' : 'ui.autoDetected') : '';
  $('detected-path').textContent = snapshot?.exe || '';
  $('error').textContent = snapshot?.error ? `${t('ui.error')} : ${snapshot.error}` : ''; $('error').hidden = !snapshot?.error;
  $('question').textContent = snapshot?.question || ''; $('question').hidden = !snapshot?.question;
  if (!initialized && snapshot) { $('setup').open = !snapshot.online; initialized = true; }
  if (snapshot && !snapshot.online) $('setup').open = true;
}
for (const id of ['language', 'mode', 'scope', 'scene', 'source', 'schedule']) {
  $(id).addEventListener('change', () => {
    settings = savedSettings.change(id, $(id).value, crypto.randomUUID());
    send('setSettings', settings); send('getSettings'); render();
  });
}
$('cli').addEventListener('input', () => { pathDirty = true; });
$('apply').addEventListener('click', () => { globalSettings = { cliPath: $('cli').value.trim() }; pathDirty = false; plugin({ type: 'connection', ...globalSettings }); });
$('retry').addEventListener('click', () => plugin({ type: 'retry' }));

// Point d'entrée envoyé une seule fois par Stream Deck. Ne jamais recharger cette page.
window.connectElgatoStreamDeckSocket = (port, uuid, event, infoJson, actionJson) => {
  const info = JSON.parse(infoJson), current = JSON.parse(actionJson);
  // Stream Deck authentifie les sorties avec l'UUID du panneau, puis relaie
  // les réponses avec le contexte de la touche. Ces identifiants sont distincts.
  inspectorContext = uuid; actionContext = current.context;
  startedAt = Date.now(); lastReply = 0; panelDisconnected = false;
  systemLanguage = info.application.language; settings = current.payload.settings || {};
  savedSettings = new InspectorSettings(settings);
  action = current.action; kind = action.split('.').at(-1); render();
  socket = new WebSocket(`ws://127.0.0.1:${port}`);
  socket.onopen = () => { socket.send(JSON.stringify({ event, uuid })); plugin({ type: 'ready' }); };
  socket.onmessage = ev => {
    try {
      const value = JSON.parse(ev.data);
      if (value.event === 'sendToPropertyInspector' && value.payload.type === 'snapshot' && value.payload.context === actionContext) {
        lastReply = Date.now(); panelDisconnected = false; snapshot = value.payload; receiveSettings(snapshot.settings);
        globalSettings = snapshot.globalSettings; kind = snapshot.kind; render();
      }
      if (value.event === 'didReceiveSettings' && value.context === actionContext) { receiveSettings(value.payload.settings); render(); }
    } catch { $('help').textContent = t('ui.noPlugin'); }
  };
  socket.onclose = () => { panelDisconnected = true; renderConnection(); };
  socket.onerror = () => { panelDisconnected = true; renderConnection(); };
};
setInterval(() => {
  plugin({ type: 'ready' });
  renderSaveStatus();
  if (Date.now() - (lastReply || startedAt) > 8000) { panelDisconnected = true; renderConnection(); }
}, 2500);
render();
