import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { spawn } from 'node:child_process';
import { WebSocketServer } from 'ws';
import path from 'node:path';

test('paquet réel : inscription SDK, touches FR/EN et dialogue du panneau', { timeout: 15000 }, async () => {
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  await once(server, 'listening');
  let child, socket, stderr = '', messages = [];
  const info = { application: { font: 'Segoe UI', language: 'fr', platform: 'windows', platformVersion: '10', version: '7.4.2' },
    plugin: { uuid: 'city.vena.polyptyque', version: '0.1.0.2' }, devicePixelRatio: 1, colors: {},
    devices: [{ id: 'device', name: 'Test', type: 0, size: { columns: 5, rows: 3 } }] };
  const wait = async predicate => {
    for (let n = 0; n < 100; n++) { const message = messages.find(predicate); if (message) return message; await new Promise(r => setTimeout(r, 40)); }
    throw new Error(`Message SDK absent. stderr=${stderr} messages=${JSON.stringify(messages).slice(-1800)}`);
  };
  server.on('connection', ws => {
    socket = ws;
    ws.on('message', raw => {
      const value = JSON.parse(raw); messages.push(value);
      if (value.event === 'getGlobalSettings') ws.send(JSON.stringify({ event: 'didReceiveGlobalSettings', id: value.id,
        payload: { settings: { cliPath: 'Z:\\__polyptyque_streamdeck_test__\\polyptyque-cli.exe' } } }));
    });
  });
  try {
    child = spawn(process.execPath, ['bin/plugin.js', '-port', String(server.address().port), '-pluginUUID', info.plugin.uuid,
      '-registerEvent', 'registerPlugin', '-info', JSON.stringify(info)],
    { cwd: path.resolve('city.vena.polyptyque.sdPlugin'), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stderr.on('data', chunk => { stderr += chunk; });
    await wait(v => v.event === 'registerPlugin');
    const action = 'city.vena.polyptyque.recording';
    const event = (name, payload = {}) => socket.send(JSON.stringify({ event: name, action, context: 'key-a', device: 'device',
      payload: { controller: 'Keypad', coordinates: { row: 0, column: 0 }, settings: {}, ...payload } }));
    event('willAppear');
    const image = await wait(v => v.event === 'setImage' && decodeURIComponent(v.payload.image).includes('Enregistrement'));
    assert.ok(image.payload.image.startsWith('data:image/svg+xml;'));
    event('propertyInspectorDidAppear');
    const panel = await wait(v => v.event === 'sendToPropertyInspector' && v.payload.reason === 'cli_missing');
    assert.equal(panel.payload.online, false); assert.equal(panel.context, 'key-a');
    event('didReceiveSettings', { settings: { language: 'en' } });
    await wait(v => v.event === 'setImage' && decodeURIComponent(v.payload.image).includes('Recording'));
    event('keyDown', { settings: { language: 'en' } });
    await wait(v => v.event === 'showAlert');
    // La pression contient les réglages persistés, même sans didReceiveSettings préalable.
    const sceneEvent = (name, settings) => socket.send(JSON.stringify({ event: name, action: 'city.vena.polyptyque.scene',
      context: 'key-scene', device: 'device', payload: { controller: 'Keypad', coordinates: { row: 0, column: 1 }, settings } }));
    sceneEvent('willAppear', { scene: 'scene-a' }); sceneEvent('propertyInspectorDidAppear', { scene: 'scene-a' });
    sceneEvent('keyDown', { scene: 'scene-b' });
    await wait(v => v.event === 'sendToPropertyInspector' && v.payload.context === 'key-scene' && v.payload.settings.scene === 'scene-b');
    assert.equal(child.exitCode, null);
  } finally { child?.kill(); socket?.terminate(); await new Promise(r => server.close(r)); }
});
