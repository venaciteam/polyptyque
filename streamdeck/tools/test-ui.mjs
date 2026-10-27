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
let peer, commands = [];
const snapshot = () => peer?.send(JSON.stringify({ event: 'sendToPropertyInspector', payload: { type: 'snapshot', context: 'key-a', kind, online,
  reason: online ? null : 'control_disabled', exe: 'R:\\Polyptyque\\current\\polyptyque-cli.exe', settings, globalSettings: {}, catalog } }));
ws.on('connection', socket => { peer = socket; socket.on('message', raw => {
  const value = JSON.parse(raw); commands.push(value);
  if (value.event === 'setSettings') settings = value.payload;
  if (value.event === 'sendToPlugin') snapshot();
}); });
let browser;
try {
  browser = await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 410, height: 1000 }, deviceScaleFactor: 1 });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/ui/panel.html`);
  await page.evaluate(port => window.connectElgatoStreamDeckSocket(String(port), 'key-a', 'registerPropertyInspector',
    JSON.stringify({ application: { language: 'fr' } }), JSON.stringify({ action: 'city.vena.polyptyque.source', payload: { settings: { source: 'source-a' } } })), ws.address().port);
  await page.waitForFunction(() => document.getElementById('status').textContent === 'Connecté à Polyptyque');
  assert.equal(await page.locator('#source').inputValue(), 'source-a');
  assert.equal(await page.locator('#source option:checked').textContent(), 'Firefox & <jeu>');
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
  await page.locator('#cli').fill('R:\\Portable\\current\\polyptyque-cli.exe'); await page.locator('#apply').click();
  await page.waitForTimeout(100);
  assert.ok(commands.some(c => c.payload?.type === 'connection' && c.payload.cliPath.startsWith('R:')));
  assert.equal(commands.filter(c => c.event === 'registerPropertyInspector').length, 1);
  assert.deepEqual(errors, []);
  await page.goto(`file://${path.resolve('../livraison/streamdeck-apercu.html').replaceAll('\\', '/')}`);
  await page.setViewportSize({ width: 1250, height: 400 });
  await page.screenshot({ path: '../livraison/streamdeck-apercu.png', fullPage: true });
  console.log('Panneau FR/EN, cibles supprimées, état hors ligne, connexion et absence de rechargement vérifiés. Captures dans livraison/.');
} finally { await browser?.close(); for (const peer of ws.clients) peer.terminate(); await new Promise(r => ws.close(r)); await new Promise(r => server.close(r)); }
