import test from 'node:test';
import assert from 'node:assert/strict';
import { command, Confirmation, duration, diskSpace, view, language } from '../src/model.mjs';
import { translate } from '../src/locales.mjs';
import { keySvg } from '../src/render.mjs';

export const snapshot = () => ({ generation: 3, reason: null, state: {
  scene: { id: 'scene-a', name: 'Podcast' }, recording: { active: false, elapsed_seconds: null, scheduled_until: null },
  sources: [{ id: 'source-a', name: 'Navigateur', recording: 'off', window: 'found' }], previews: true,
  disk: { free_bytes: 412300000000, hours_left: 49.3 }
}, catalog: { scenes: [{ id: 'scene-a', name: 'Podcast' }, { id: 'scene-b', name: 'Jeu' }],
  sources: [], schedules: [{ id: 'schedule-a', scene: { name: 'Podcast' }, start: '20:00', end: '21:00', enabled: true }] } });

test('toutes les actions suivent l’état et les identifiants, avec modes explicites', () => {
  const s = snapshot();
  assert.deepEqual(command('recording', {}, s), { method: 'recording.start', params: {} });
  s.state.recording.active = true;
  assert.deepEqual(command('recording', {}, s), { method: 'recording.stop', params: {} });
  assert.equal(command('recording', { mode: 'on' }, s), null);
  assert.deepEqual(command('source', { source: 'source-a' }, s), { method: 'recording.start', params: { source: 'source-a' } });
  s.state.sources[0].recording = 'recording';
  assert.equal(command('source', { source: 'source-a', mode: 'on' }, s), null);
  assert.deepEqual(command('source', { source: 'source-a' }, s), { method: 'recording.stop', params: { source: 'source-a' } });
  assert.equal(command('scene', { scene: 'scene-a' }, s), null);
  assert.deepEqual(command('scene', { scene: 'scene-b' }, s), { method: 'scenes.activate', params: { scene: 'scene-b' } });
  assert.deepEqual(command('previews', {}, s), { method: 'previews.set', params: { enabled: false } });
  assert.equal(command('previews', { mode: 'on' }, s), null);
  assert.deepEqual(command('schedule', { schedule: 'schedule-a' }, s), { method: 'schedules.enable', params: { schedule: 'schedule-a', enabled: false } });
  assert.deepEqual(command('windows', {}, s), { method: 'scenes.open_windows', params: {} });
  s.state.sources[0].window = 'minimized';
  assert.equal(command('windows', { scope: 'source', source: 'source-a' }, s).method, 'sources.restore_window');
  s.state.sources[0].window = 'missing';
  assert.equal(command('windows', { scope: 'source', source: 'source-a' }, s).method, 'sources.open_window');
  assert.equal(command('disk', {}, s), null);
});
test('une cible disparue ou une connexion coupée ne produit aucune commande de secours', () => {
  for (const [kind, settings] of [['source', { source: 'deleted' }], ['scene', { scene: 'deleted' }], ['schedule', { schedule: 'deleted' }]]) {
    assert.throws(() => command(kind, settings, snapshot()), e => ['source_inactive', 'target_missing'].includes(e.reason));
  }
  const s = snapshot(); s.state = null; s.reason = 'control_disabled';
  assert.throws(() => command('recording', {}, s), e => e.reason === 'control_disabled');
  assert.equal(view('recording', {}, s, k => translate('en', k)).tone, 'offline');
});
test('confirmation limitée à une pression, cinq secondes et au contexte d’origine', () => {
  const s = snapshot(), c = new Confirmation(), request = { method: 'recording.stop', params: {} };
  c.remember(request, {}, s, 100); assert.ok(c.take(request, {}, s, 5099)); assert.equal(c.take(request, {}, s, 5100), false);
  c.remember(request, {}, s, 100); assert.equal(c.take(request, {}, s, 5100), false);
  for (const alter of [v => v.generation++, v => v.state.scene.id = 'other', v => v.state.recording.scheduled_until = 'other']) {
    const next = snapshot(); c.remember(request, {}, next, 100); alter(next); assert.equal(c.take(request, {}, next, 101), false);
  }
  c.remember(request, {}, s, 100); assert.equal(c.take(request, { mode: 'off' }, s, 101), false);
});
test('durée, disque, fenêtre perdue et échappement des libellés arbitraires', () => {
  assert.equal(duration(754), '12:34'); assert.equal(duration(45296), '12:34:56');
  assert.equal(diskSpace(null, 'fr'), '—'); assert.equal(diskSpace(412300000000, 'fr'), '412,3 Go');
  assert.equal(language('auto', 'fr-FR'), 'fr'); assert.equal(language('auto', 'de'), 'en');
  const s = snapshot(); s.state.sources[0].window = 'missing';
  assert.equal(view('source', { source: 'source-a' }, s, k => translate('fr', k)).tone, 'warning');
  const svg = keySvg({ title: '<script>&"', value: '中文日本語 한글 🎮', tone: 'warning' });
  assert.ok(!svg.includes('<script>')); assert.ok(svg.includes('&lt;script&gt;'));
});
