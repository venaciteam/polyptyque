import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { PassThrough } from 'node:stream';
import { JsonLines, PolyptyqueClient, runCli } from '../src/client.mjs';

test('lignes fragmentées, regroupées, UTF-8 et limite de mémoire', () => {
  const values = [], parser = new JsonLines(v => values.push(v));
  parser.push('{"text":"Sc'); parser.push('ène 🎙"}\n{"a":1}\n');
  assert.deepEqual(values, [{ text: 'Scène 🎙' }, { a: 1 }]);
  assert.throws(() => parser.push('x'.repeat(2 * 1024 * 1024 + 1)), e => e.reason === 'invalid_response');
});
test('exécution sans shell et récupération des codes d’erreur JSON malgré un code de sortie non nul', async () => {
  const value = await runCli(process.execPath, ['-e', 'console.log(JSON.stringify({name:process.argv[1]}))', 'Scène & $(test) "🎙"']);
  assert.equal(value.name, 'Scène & $(test) "🎙"');
  await assert.rejects(runCli(process.execPath, ['-e', 'console.log(JSON.stringify({error:{reason:"confirmation_required",message:"Stop?",question:"Stop?"}}));process.exit(7)']),
    e => e.reason === 'confirmation_required' && e.question === 'Stop?');
});
const state = { scene: { id: 'a' }, sources: [], recording: { active: false } };
function fakeChild() {
  const child = new EventEmitter(); child.stdout = new PassThrough(); child.kill = () => {};
  return child;
}
test('connexion partagée, coupure efface l’état, reconnexion et aucune répétition de commande', async () => {
  const children = [], calls = [];
  let resolveCommand;
  const client = new PolyptyqueClient({ locate: async () => 'fake.exe', retryMs: 15, catalogMs: 10000,
    launch: () => { const child = fakeChild(); children.push(child); return child; },
    run: async (exe, args) => { calls.push(args[1]); if (args[1] === 'recording.start') return new Promise(r => { resolveCommand = r; }); return { scenes: [], sources: [], schedules: [] }; } });
  try {
    client.start(); await new Promise(r => setImmediate(r));
    children[0].stdout.write(JSON.stringify({ event: 'state', data: state }) + '\n');
    assert.equal(client.reason, null); assert.deepEqual(client.state, state);
    const pending = client.call('recording.start');
    children[0].emit('close'); assert.equal(client.state, null);
    resolveCommand({ started: true }); await assert.rejects(pending, e => e.reason === 'connection_changed');
    await new Promise(r => setTimeout(r, 40)); assert.equal(children.length, 2);
    children[1].stdout.write(JSON.stringify({ event: 'state', data: state }) + '\n');
    assert.deepEqual(client.state, state); assert.equal(calls.filter(c => c === 'recording.start').length, 1);
    children[0].stdout.write(JSON.stringify({ event: 'state', data: { ...state, scene: { id: 'stale' } } }) + '\n');
    assert.equal(client.state.scene.id, 'a');
  } finally { client.stop(); }
});
test('pilotage désactivé et réponse incorrecte restent hors ligne', async () => {
  let child;
  const client = new PolyptyqueClient({ locate: async () => 'fake.exe', retryMs: 10000, launch: () => (child = fakeChild()) });
  try {
    client.start(); await new Promise(r => setImmediate(r));
    child.stdout.write('{"error":{"reason":"control_disabled"}}\n');
    assert.equal(client.state, null); assert.equal(client.reason, 'control_disabled');
  } finally { client.stop(); }
});
test('le panneau ne reçoit ni URL de navigation ni contenu complet des scènes', async () => {
  let child;
  const client = new PolyptyqueClient({ locate: async () => 'fake.exe', catalogMs: 10000,
    launch: () => (child = fakeChild()), run: async (exe, args, timeout) => {
      assert.equal(timeout, 45000);
      if (args[1] === 'scenes.list') return { scenes: [{ id: 'a', name: 'Scène', sources: [{ start_url: 'https://private' }] }] };
      if (args[1] === 'sources.list') return { sources: [{ id: 's', name: 'Source', scenes: ['a'], start_url: 'https://private' }] };
      return { schedules: [] };
    } });
  try {
    client.start(); await new Promise(r => setImmediate(r));
    child.stdout.write(JSON.stringify({ event: 'state', data: state }) + '\n');
    await new Promise(r => setImmediate(r));
    assert.deepEqual(client.catalog, { scenes: [{ id: 'a', name: 'Scène' }], sources: [{ id: 's', name: 'Source', scenes: ['a'] }], schedules: [] });
  } finally { client.stop(); }
});
test('une application figée ne conserve pas un faux état en ligne', async () => {
  let child;
  const client = new PolyptyqueClient({ locate: async () => 'fake.exe', retryMs: 10000,
    launch: () => (child = fakeChild()), run: async () => { throw Object.assign(new Error('Timeout'), { reason: 'timeout' }); } });
  try {
    client.start(); await new Promise(r => setImmediate(r));
    child.stdout.write(JSON.stringify({ event: 'state', data: state }) + '\n');
    await new Promise(r => setImmediate(r));
    assert.equal(client.state, null); assert.equal(client.reason, 'timeout');
  } finally { client.stop(); }
});
