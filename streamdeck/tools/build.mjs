import { build } from 'esbuild';
import { copyFile, cp, mkdir, writeFile, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { kinds, view } from '../src/model.mjs';
import { fr, en, translate } from '../src/locales.mjs';
import { keySvg } from '../src/render.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
process.chdir(root);
const out = path.join(root, 'city.vena.polyptyque.sdPlugin');
for (const dir of ['bin', 'ui', 'images']) await mkdir(path.join(out, dir), { recursive: true });
await build({ entryPoints: ['src/plugin.mjs'], outfile: `${out}/bin/plugin.js`, bundle: true, platform: 'node', target: 'node24', format: 'esm',
  banner: { js: 'import { createRequire as __polyptyqueRequire } from "node:module"; const require = __polyptyqueRequire(import.meta.url);' } });
await build({ entryPoints: ['src/panel.mjs'], outfile: `${out}/ui/panel.js`, bundle: true, platform: 'browser', target: 'chrome120' });
await cp('ui', `${out}/ui`, { recursive: true });
await copyFile('../assets/logo/polyptyque-512.png', `${out}/images/plugin@2x.png`);
await copyFile('../crates/polyptyque-app/ui/icons/polyptyque.png', `${out}/images/plugin.png`);
await copyFile('../LICENSE', `${out}/LICENSE`);
await writeFile(`${out}/package.json`, JSON.stringify({ type: 'module', version: '0.1.0' }));
await writeFile(`${out}/THIRD-PARTY.txt`, `Polyptyque: GPL-3.0-or-later (LICENSE).\n@elgato/streamdeck, @elgato/utils, ws, zod: MIT.\nSource: https://github.com/venaciteam/polyptyque/tree/dev/streamdeck\n\n` +
  (await Promise.all(['@elgato/streamdeck', '@elgato/utils', 'ws', 'zod'].map(async name => `${name}\n${await readFile(`node_modules/${name}/LICENSE`, 'utf8')}`))).join('\n\n'));
const symbols = {
  recording: '<circle cx="10" cy="10" r="7" fill="white"/>',
  source: '<rect x="2" y="4" width="16" height="12" rx="2"/><circle cx="10" cy="10" r="3" fill="white"/>',
  scene: '<rect x="1" y="3" width="5" height="14" rx="1"/><rect x="8" y="3" width="5" height="14" rx="1"/><rect x="15" y="3" width="4" height="14" rx="1"/>',
  windows: '<rect x="1" y="5" width="14" height="12" rx="1"/><path d="M5 5V2h14v11h-4M1 8h14"/>',
  previews: '<path d="M1 10s3-6 9-6 9 6 9 6-3 6-9 6-9-6-9-6Z"/><circle cx="10" cy="10" r="3"/>',
  schedule: '<rect x="2" y="4" width="16" height="14" rx="2"/><path d="M2 8h16M6 1v5M14 1v5M10 10v4h3"/>',
  disk: '<rect x="2" y="3" width="16" height="14" rx="2"/><path d="M3 12h14M13 15h2"/>'
};
for (const [name, symbol] of Object.entries(symbols)) {
  await writeFile(`${out}/images/${name}.svg`, `<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20"><g stroke="white" stroke-width="1.5" fill="none" stroke-linejoin="round">${symbol}</g></svg>`);
  await writeFile(`${out}/images/${name}-key.svg`, keySvg({ title: fr.action[name], value: 'Polyptyque' }));
}
await writeFile(`${out}/images/category.svg`, '<svg xmlns="http://www.w3.org/2000/svg" width="28" height="28" viewBox="0 0 28 28"><g fill="white"><rect x="3" y="5" width="5" height="18" rx="1"/><rect x="11" y="5" width="5" height="18" rx="1"/><rect x="19" y="5" width="5" height="18" rx="1"/></g></svg>');
const manifest = {
  $schema: 'https://schemas.elgato.com/streamdeck/plugins/manifest.json', UUID: 'city.vena.polyptyque', Name: 'Polyptyque', Version: '0.1.0.1',
  Author: 'Venacity', Description: 'Piloter Polyptyque et suivre les enregistrements depuis Stream Deck.', URL: 'https://github.com/venaciteam/polyptyque',
  Category: 'Polyptyque', CategoryIcon: 'images/category', Icon: 'images/plugin', CodePath: 'bin/plugin.js', PropertyInspectorPath: 'ui/panel.html',
  SDKVersion: 3, Software: { MinimumVersion: '7.1' }, OS: [{ Platform: 'windows', MinimumVersion: '10' }], Nodejs: { Version: '24' },
  Actions: kinds.map(kind => ({ UUID: `city.vena.polyptyque.${kind}`, Name: fr.action[kind], Tooltip: fr.tooltip[kind],
    Icon: `images/${kind}`, Controllers: ['Keypad'], DisableAutomaticStates: true, SupportedInMultiActions: false,
    States: [{ Image: `images/${kind}-key`, ShowTitle: false }] }))
};
await writeFile(`${out}/manifest.json`, JSON.stringify(manifest, null, 2));
for (const [lang, strings] of Object.entries({ fr, en })) {
  await writeFile(`${out}/${lang}.json`, JSON.stringify({ Name: 'Polyptyque', Description: lang === 'fr' ? manifest.Description : 'Control Polyptyque and monitor recordings from Stream Deck.',
    ...Object.fromEntries(kinds.map(kind => [`city.vena.polyptyque.${kind}`, { Name: strings.action[kind], Tooltip: strings.tooltip[kind] }])), Localization: strings }, null, 2));
}
// Planche statique : cas réels et limites, sans connexion ni mutation de Polyptyque.
const cases = [
  { title: 'Enregistrement', value: '12:34:56', detail: 'En cours', tone: 'recording' },
  { title: 'Navigateur très long & jeu', value: '3:42', detail: 'Fenêtre perdue', tone: 'warning' },
  { title: 'Confirmer', value: 'Appuyer à nouveau', detail: '5', tone: 'warning' },
  { title: 'Espace disque', value: '412,3 Go', detail: '49.3 h', tone: 'normal' },
  { title: 'Recording', value: 'Polyptyque closed', tone: 'offline' },
  { title: 'Scheduled recording', value: 'Paused', detail: '20:00–21:30' },
  { title: '中文日本語 한글 🎮 & < >', value: '12:34:56', detail: 'Window lost', tone: 'warning' }
];
await mkdir('../livraison', { recursive: true });
await writeFile('../livraison/streamdeck-apercu.html', `<!doctype html><meta charset="utf-8"><title>Polyptyque - Stream Deck</title><body style="background:#0e1014;color:#e8eaee;font:16px Segoe UI;padding:28px"><h1>Polyptyque · Stream Deck</h1><div style="display:flex;gap:18px;flex-wrap:wrap">${cases.map(keySvg).join('')}</div><p>États de contrôle · français et anglais</p>`);
console.log('Plugin construit :', out);
