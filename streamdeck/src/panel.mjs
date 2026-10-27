import { translate } from './locales.mjs';
import { language } from './model.mjs';

let socket, context, action, kind, systemLanguage = 'fr', settings = {}, globalSettings = {}, snapshot;
let lastReply = 0, initialized = false, pathDirty = false;
const $ = id => document.getElementById(id);
const t = key => translate(language(settings.language, systemLanguage), key);
function send(event, payload) {
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ event, context, ...(event === 'sendToPlugin' ? { action } : {}), payload }));
}
const plugin = payload => send('sendToPlugin', payload);
function localize() {
  document.documentElement.lang = language(settings.language, systemLanguage);
  for (const node of document.querySelectorAll('[data-i18n]')) node.textContent = t(node.dataset.i18n);
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
  $('connection').classList.toggle('online', Boolean(snapshot?.online));
  $('status').textContent = snapshot?.online ? t('ui.connected') : snapshot ? t(`error.${snapshot.reason || 'not_running'}`) : t('ui.waiting');
  const helpKey = snapshot?.reason && `ui.${snapshot.reason}`;
  $('help').textContent = helpKey && t(helpKey) !== helpKey ? t(helpKey) : '';
  if (!pathDirty) $('cli').value = globalSettings.cliPath || '';
  $('detected').textContent = snapshot?.exe || '';
  $('error').textContent = snapshot?.error ? `${t('ui.error')} : ${snapshot.error}` : ''; $('error').hidden = !snapshot?.error;
  $('question').textContent = snapshot?.question || ''; $('question').hidden = !snapshot?.question;
  if (!initialized && snapshot) { $('setup').open = !snapshot.online; initialized = true; }
  if (snapshot && !snapshot.online) $('setup').open = true;
}
for (const id of ['language', 'mode', 'scope', 'scene', 'source', 'schedule']) {
  $(id).addEventListener('change', () => { settings = { ...settings, [id]: $(id).value }; send('setSettings', settings); render(); });
}
$('cli').addEventListener('input', () => { pathDirty = true; });
$('apply').addEventListener('click', () => { globalSettings = { cliPath: $('cli').value.trim() }; pathDirty = false; plugin({ type: 'connection', ...globalSettings }); });
$('retry').addEventListener('click', () => plugin({ type: 'retry' }));

// Point d'entrée envoyé une seule fois par Stream Deck. Ne jamais recharger cette page.
window.connectElgatoStreamDeckSocket = (port, uuid, event, infoJson, actionJson) => {
  context = uuid;
  const info = JSON.parse(infoJson), current = JSON.parse(actionJson);
  systemLanguage = info.application.language; settings = current.payload.settings || {};
  action = current.action; kind = action.split('.').at(-1); render();
  socket = new WebSocket(`ws://127.0.0.1:${port}`);
  socket.onopen = () => { socket.send(JSON.stringify({ event, uuid })); plugin({ type: 'ready' }); };
  socket.onmessage = ev => {
    try {
      const value = JSON.parse(ev.data);
      if (value.event === 'sendToPropertyInspector' && value.payload.type === 'snapshot' && value.payload.context === context) {
        lastReply = Date.now(); snapshot = value.payload; settings = snapshot.settings;
        globalSettings = snapshot.globalSettings; kind = snapshot.kind; render();
      }
      if (value.event === 'didReceiveSettings') { settings = value.payload.settings; render(); }
    } catch { $('help').textContent = t('ui.noPlugin'); }
  };
  socket.onclose = () => { $('connection').classList.remove('online'); $('status').textContent = t('ui.noPlugin'); };
};
setInterval(() => {
  plugin({ type: 'ready' });
  if (!lastReply || Date.now() - lastReply > 8000) { $('connection').classList.remove('online'); $('help').textContent = t('ui.noPlugin'); }
}, 2500);
render();
