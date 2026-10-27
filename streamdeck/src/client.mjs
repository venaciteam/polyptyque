import { EventEmitter } from 'node:events';
import { execFile, spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { failure } from './errors.mjs';

const exec = promisify(execFile);
const LIMIT = 2 * 1024 * 1024;

// Le CLI conserve la vérification du propriétaire du tube, sans FFI dans le plugin.
export async function runCli(exe, args, timeout = 320000) {
  let stdout;
  try { ({ stdout } = await exec(exe, args, { windowsHide: true, timeout, maxBuffer: LIMIT, encoding: 'utf8' })); }
  catch (error) {
    try {
      const value = JSON.parse(error.stdout);
      if (value.error) throw Object.assign(new Error(value.error.message), value.error);
    } catch (parsed) { if (parsed.reason) throw parsed; }
    throw failure(error.killed ? 'timeout' : 'unavailable');
  }
  let value;
  try { value = JSON.parse(stdout); } catch { throw failure('invalid_response'); }
  if (value.error) throw Object.assign(new Error(value.error.message), value.error);
  return value;
}

async function exists(file) { try { await access(file); return true; } catch { return false; } }
export async function discoverCli(manual = '') {
  if (manual.trim()) {
    const file = manual.trim().replace(/^"|"$/g, '');
    if (!path.isAbsolute(file) || path.basename(file).toLowerCase() !== 'polyptyque-cli.exe' || !await exists(file)) {
      throw failure('cli_missing');
    }
    return file;
  }
  // L'application ouverte est prioritaire : portable et installation personnalisée compris.
  try {
    const { stdout } = await exec('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
      '[Console]::OutputEncoding = [Text.UTF8Encoding]::new(); Get-Process -Name polyptyque -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Path -Unique'],
    { windowsHide: true, timeout: 6000, maxBuffer: 65536, encoding: 'utf8' });
    for (const line of stdout.trim().split(/\r?\n/)) {
      if (!line || !path.isAbsolute(line)) continue;
      const candidate = path.join(path.dirname(line), 'polyptyque-cli.exe');
      if (await exists(candidate)) return candidate;
    }
  } catch { /* Une installation standard reste détectable si Polyptyque est fermé. */ }
  for (const base of [process.env.LOCALAPPDATA, process.env.ProgramFiles]) {
    if (!base) continue;
    for (const folder of ['PolyptyqueApp', 'Polyptyque']) {
      const candidate = path.join(base, folder, 'current', 'polyptyque-cli.exe');
      if (await exists(candidate)) return candidate;
    }
  }
  throw failure('cli_missing');
}

// Un tampon borné protège aussi contre une sortie cassée ou un mauvais exécutable.
export class JsonLines {
  constructor(consume) { this.consume = consume; this.buffer = ''; }
  push(chunk) {
    this.buffer += chunk;
    if (Buffer.byteLength(this.buffer) > LIMIT) throw failure('invalid_response');
    let end;
    while ((end = this.buffer.indexOf('\n')) !== -1) {
      const line = this.buffer.slice(0, end).trim();
      this.buffer = this.buffer.slice(end + 1);
      if (line) this.consume(JSON.parse(line));
    }
  }
}

export class PolyptyqueClient extends EventEmitter {
  constructor({ locate = discoverCli, run = runCli, launch = spawn, retryMs = 4000, catalogMs = 4000 } = {}) {
    super(); Object.assign(this, { locate, run, launch, retryMs, catalogMs });
    this.state = null; this.catalog = { scenes: [], sources: [], schedules: [] };
    this.reason = 'connecting'; this.exe = ''; this.manual = ''; this.generation = 0; this.closed = true;
  }
  snapshot() { return { state: this.state, catalog: this.catalog, reason: this.reason, exe: this.exe, generation: this.generation }; }
  changed() { this.emit('change', this.snapshot()); }
  start(manual = '') { this.stop(); this.manual = manual; this.closed = false; void this.connect(this.generation); }
  stop() {
    this.closed = true; this.generation++;
    clearTimeout(this.retry); clearTimeout(this.catalogTimer); this.child?.kill(); this.child = null;
    this.state = null; this.catalog = { scenes: [], sources: [], schedules: [] }; this.reason = 'connecting';
    this.changed();
  }
  async connect(generation) {
    if (this.closed || generation !== this.generation) return;
    try {
      const exe = await this.locate(this.manual);
      if (this.closed || generation !== this.generation) return;
      this.exe = exe;
      const child = this.launch(exe, ['events'], { windowsHide: true, stdio: ['ignore', 'pipe', 'ignore'] });
      this.child = child;
      let ended = false;
      const finish = (reason) => {
        if (ended || generation !== this.generation || this.closed) return;
        ended = true; clearTimeout(initial); clearTimeout(this.catalogTimer);
        child.kill(); this.child = null; this.state = null; this.reason = reason;
        this.catalog = { scenes: [], sources: [], schedules: [] }; this.generation++; this.changed();
        this.retry = setTimeout(() => void this.connect(this.generation), this.retryMs);
      };
      const initial = setTimeout(() => finish('timeout'), 15000);
      const lines = new JsonLines(value => {
        if (ended || generation !== this.generation) return;
        if (value.error) return finish(value.error.reason ?? 'unavailable');
        if (value.event === 'state') {
          if (!value.data?.scene?.id || !Array.isArray(value.data.sources) || !value.data.recording) return finish('invalid_response');
          const first = !this.state;
          clearTimeout(initial); this.reason = null; this.state = value.data; this.changed();
          if (first) void this.refreshCatalog(generation);
        }
      });
      child.stdout.setEncoding('utf8');
      child.stdout.on('data', chunk => { try { lines.push(chunk); } catch { finish('invalid_response'); } });
      child.once('error', () => finish('unavailable'));
      child.once('close', () => finish('not_running'));
    } catch (error) {
      if (this.closed || generation !== this.generation) return;
      this.state = null; this.reason = error.reason ?? 'unavailable'; this.changed();
      this.retry = setTimeout(() => void this.connect(generation), this.retryMs);
    }
  }
  async call(method, params = {}) {
    if (!this.state) throw failure(this.reason || 'not_running');
    const generation = this.generation;
    const value = await this.run(this.exe, ['call', method, JSON.stringify(params)]);
    // Jamais de répétition automatique : une commande peut avoir été exécutée avant la coupure.
    if (generation !== this.generation || !this.state) throw failure('connection_changed');
    return value;
  }
  async refreshCatalog(generation = this.generation) {
    if (generation !== this.generation || !this.state || this.closed) return;
    try {
      const [scenes, sources, schedules] = await Promise.all([
        this.call('scenes.list'), this.call('sources.list', { all: true }), this.call('schedules.list')]);
      if (generation !== this.generation) return;
      this.catalog = { scenes: scenes.scenes, sources: sources.sources, schedules: schedules.schedules };
      this.changed();
    } catch { /* La connexion d'événements reste la source de vérité pour l'état en ligne. */ }
    finally {
      if (generation === this.generation && !this.closed && this.state) {
        this.catalogTimer = setTimeout(() => void this.refreshCatalog(generation), this.catalogMs);
      }
    }
  }
}
