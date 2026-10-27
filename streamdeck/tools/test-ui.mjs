import { chromium } from 'playwright-core';
import { WebSocketServer } from 'ws';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';

const root = path.resolve('city.vena.polyptyque.sdPlugin');
const server = createServer(async (req, res) => {
  try {
    const file = path.resolve(root, `.${decodeURIComponent(req.url.split('?')[0])}`);
    if (!file.startsWith(`${root}${path.sep}`)) { res.writeHead(403).end(); return; }
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : file.endsWith('.png') ? 'image/png' : 'text/html');
    res.end(await readFile(file));
  } catch { res.writeHead(404).end(); }
});
server.listen(0, '127.0.0.1'); await once(server, 'listening');
const ws = new WebSocketServer({ host: '127.0.0.1', port: 0 }); await once(ws, 'listening');
let settings = { source: 'source-a' }, kind = 'source', online = true, catalog = {
  scenes: [{ id: 'scene-a', name: 'Podcast' }], sources: [{ id: 'source-a', name: 'Firefox & <jeu>' }],
  schedules: [{ id: 'schedule-a', scene: { name: 'Podcast' }, start: '20:00', end: '21:00', days: ['mon', 'fri'], enabled: true }]
};
let peer, commands = [], respond = true;
const snapshot = () => peer?.send(JSON.stringify({ event: 'sendToPropertyInspector', payload: { type: 'snapshot', context: 'key-a', kind, online,
  reason: online ? null : 'control_disabled', exe: 'R:\\Polyptyque\\current\\polyptyque-cli.exe', settings, globalSettings: {}, catalog } }));
ws.on('connection', socket => { peer = socket; socket.on('message', raw => {
  const value = JSON.parse(raw); commands.push(value);
  if (value.event === 'setSettings' && value.context === 'key-a' && value.action) settings = value.payload;
  if (respond && value.event === 'sendToPlugin' && value.context === 'key-a') snapshot();
}); });
let browser;
try {
  browser = await chromium.launch({ channel: process.env.POLYPTYQUE_TEST_BROWSER || 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 410, height: 1000 }, deviceScaleFactor: 1 });
  page.setDefaultTimeout(5000);
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/ui/panel.html`);
  await page.evaluate(port => window.connectElgatoStreamDeckSocket(String(port), 'inspector-session-a', 'registerPropertyInspector',
    JSON.stringify({ application: { language: 'fr' } }), JSON.stringify({ context: 'key-a', action: 'city.vena.polyptyque.source', payload: { settings: { source: 'source-a' } } })), ws.address().port);
  await page.waitForFunction(() => document.getElementById('status').textContent === 'Connecté à Polyptyque');
  // Une réponse d'une autre touche ne doit pas remplacer celle du panneau courant.
  peer.send(JSON.stringify({ event: 'sendToPropertyInspector', payload: { type: 'snapshot', context: 'other-key', online: false } }));
  await page.waitForTimeout(50);
  assert.equal(await page.locator('#status').textContent(), 'Connecté à Polyptyque');
  // La perte du dialogue du panneau ne doit conserver ni une attente infinie ni un faux vert.
  respond = false; await page.clock.install(); await page.clock.fastForward(10000);
  await page.waitForFunction(() => document.getElementById('status').textContent === 'Panneau déconnecté');
  assert.equal(await page.locator('#connection').evaluate(node => node.classList.contains('online')), false);
  respond = true; snapshot();
  await page.waitForFunction(() => document.getElementById('status').textContent === 'Connecté à Polyptyque');
  assert.equal(await page.locator('#source').inputValue(), 'source-a');
  assert.equal(await page.locator('#source option:checked').textContent(), 'Firefox & <jeu>');
  assert.equal(await page.locator('#detected').textContent(), 'Polyptyque détecté automatiquement.');
  await page.locator('#setup > summary').click();
  assert.equal(await page.locator('#cli').isVisible(), false);
  await page.locator('#setup > summary').click();
  await page.screenshot({ path: '../livraison/streamdeck-reglages-fr.png', fullPage: true });
  await page.locator('#language').selectOption('en');
  await page.waitForFunction(() => document.getElementById('status').textContent === 'Connected to Polyptyque');
  assert.equal(await page.locator('#source').inputValue(), 'source-a');
  await page.screenshot({ path: '../livraison/streamdeck-settings-en.png', fullPage: true });
  catalog.sources = []; snapshot();
  await page.waitForFunction(() => document.getElementById('source').selectedOptions[0].textContent.includes('Saved target not found'));
  assert.equal(await page.locator('#source').inputValue(), 'source-a');
  kind = 'windows'; settings = { language: 'fr', scope: 'source', source: 'source-a' }; snapshot();
  await page.waitForFunction(() => !document.getElementById('scope-field').hidden);
  await page.locator('#scope').selectOption('scene');
  assert.equal(await page.locator('#source-field').isHidden(), true);
  online = false; snapshot();
  await page.waitForFunction(() => document.getElementById('status').textContent === 'Pilotage désactivé');
  assert.equal(await page.locator('#setup').getAttribute('open'), '');
  await page.screenshot({ path: '../livraison/streamdeck-connexion-fr.png', fullPage: true });
  await page.locator('#manual > summary').click();
  await page.locator('#cli').fill('R:\\Portable\\current\\polyptyque-cli.exe'); await page.locator('#apply').click();
  await page.waitForTimeout(100);
  assert.ok(commands.some(c => c.payload?.type === 'connection' && c.payload.cliPath.startsWith('R:')));
  assert.equal(commands.filter(c => c.event === 'registerPropertyInspector').length, 1);
  assert.equal(commands.find(c => c.event === 'registerPropertyInspector').uuid, 'inspector-session-a');
  assert.ok(commands.filter(c => ['sendToPlugin', 'setSettings'].includes(c.event)).every(c => c.context === 'key-a' && c.action.startsWith('city.vena.polyptyque.')));
  assert.deepEqual(errors, []);
  await page.goto(`file://${path.resolve('../livraison/streamdeck-apercu.html').replaceAll('\\', '/')}`);
  await page.setViewportSize({ width: 1250, height: 400 });
  await page.screenshot({ path: '../livraison/streamdeck-apercu.png', fullPage: true });
  console.log('Panneau FR/EN, cibles supprimées, état hors ligne, connexion et absence de rechargement vérifiés. Captures dans livraison/.');
} finally { await browser?.close(); for (const peer of ws.clients) peer.terminate(); await new Promise(r => ws.close(r)); await new Promise(r => server.close(r)); }
