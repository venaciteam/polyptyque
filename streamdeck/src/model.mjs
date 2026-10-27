import { failure } from './errors.mjs';

export const kinds = ['recording', 'source', 'scene', 'windows', 'previews', 'schedule', 'disk'];
export const language = (choice, system = 'en') => (choice === 'fr' || choice === 'en') ? choice : (system.startsWith('fr') ? 'fr' : 'en');
export const enabled = (mode, current) => mode === 'on' ? true : mode === 'off' ? false : !current;
export const recording = source => ['starting', 'recording', 'finishing'].includes(source?.recording);
export function duration(seconds) {
  seconds = Math.max(0, Math.floor(Number(seconds) || 0));
  const m = Math.floor(seconds / 60), s = String(seconds % 60).padStart(2, '0');
  return m >= 60 ? `${Math.floor(m / 60)}:${String(m % 60).padStart(2, '0')}:${s}` : `${m}:${s}`;
}
export function diskSpace(bytes, lang) {
  if (!Number.isFinite(bytes)) return '—';
  const unit = bytes >= 1e12 ? 1e12 : 1e9;
  return `${new Intl.NumberFormat(lang, { maximumFractionDigits: 1 }).format(bytes / unit)} ${lang === 'fr' ? (unit === 1e12 ? 'To' : 'Go') : (unit === 1e12 ? 'TB' : 'GB')}`;
}
export function sourceTarget(state, settings) {
  const source = state.sources.find(s => s.id === settings.source);
  if (!source) throw failure(settings.source ? 'source_inactive' : 'configure');
  return source;
}

// Construit une seule commande à partir de l'état courant, jamais d'un état optimiste.
export function command(kind, settings, snapshot) {
  const { state, catalog } = snapshot;
  if (!state) throw failure(snapshot.reason || 'not_running');
  if (state.busy && kind !== 'disk') throw failure('busy');
  let source;
  switch (kind) {
    case 'recording': case 'source': {
      source = kind === 'source' ? sourceTarget(state, settings) : null;
      const active = source ? recording(source) : state.recording.active;
      const start = enabled(settings.mode, active);
      if (start === active) return null;
      return { method: start ? 'recording.start' : 'recording.stop', params: source ? { source: source.id } : {} };
    }
    case 'scene':
      if (!settings.scene) throw failure('configure');
      if (!catalog.scenes.some(s => s.id === settings.scene)) throw failure('target_missing');
      return state.scene.id === settings.scene ? null : { method: 'scenes.activate', params: { scene: settings.scene } };
    case 'windows':
      if (settings.scope === 'source') {
        source = sourceTarget(state, settings);
        return { method: source.window === 'minimized' ? 'sources.restore_window' : 'sources.open_window', params: { source: source.id } };
      }
      return { method: 'scenes.open_windows', params: {} };
    case 'previews': {
      const next = enabled(settings.mode, state.previews);
      return next === state.previews ? null : { method: 'previews.set', params: { enabled: next } };
    }
    case 'schedule': {
      if (!settings.schedule) throw failure('configure');
      const schedule = catalog.schedules.find(s => s.id === settings.schedule);
      if (!schedule) throw failure('target_missing');
      const next = enabled(settings.mode, schedule.enabled);
      return next === schedule.enabled ? null : { method: 'schedules.enable', params: { schedule: schedule.id, enabled: next } };
    }
    case 'disk': return null;
    default: throw failure('configure');
  }
}

export function view(kind, settings, snapshot, t) {
  const { state, catalog } = snapshot;
  const title = t(`action.${kind}`);
  if (!state) return { title, value: t(`error.${snapshot.reason || 'not_running'}`), tone: 'offline' };
  const source = state.sources.find(s => s.id === settings.source);
  switch (kind) {
    case 'recording':
      return { title, value: state.recording.active ? duration(state.recording.elapsed_seconds) : t('key.ready'),
        detail: state.recording.active ? t('key.recording') : state.scene.name, tone: state.recording.active ? 'recording' : 'normal' };
    case 'source':
      if (!source) return { title, value: t(settings.source ? 'error.source_inactive' : 'error.configure'), tone: 'warning' };
      return { title: source.name, value: source.recording === 'failed' ? t('key.failed') : recording(source) ? duration(state.recording.elapsed_seconds) : t('key.ready'),
        detail: t(`window.${source.window}`), tone: ['missing', 'minimized'].includes(source.window) || source.recording === 'failed' ? 'warning' : recording(source) ? 'recording' : 'normal' };
    case 'scene': {
      const scene = catalog.scenes.find(s => s.id === settings.scene);
      return { title, value: scene?.name || t(settings.scene ? 'error.target_missing' : 'error.configure'),
        detail: scene?.id === state.scene.id ? t('key.active') : '', tone: !scene ? 'warning' : scene.id === state.scene.id ? 'active' : 'normal' };
    }
    case 'windows':
      return settings.scope === 'source' ? { title: source?.name || title, value: t(source ? `window.${source.window}` : settings.source ? 'error.source_inactive' : 'error.configure'),
        tone: !source || ['missing', 'minimized'].includes(source.window) ? 'warning' : 'normal' } :
        { title, value: `${state.sources.filter(s => s.window === 'found').length}/${state.sources.length}`, detail: state.scene.name,
          tone: state.sources.some(s => ['missing', 'minimized'].includes(s.window)) ? 'warning' : 'normal' };
    case 'previews': return { title, value: t(state.previews ? 'key.on' : 'key.off'), tone: state.previews ? 'active' : 'normal' };
    case 'schedule': {
      const schedule = catalog.schedules.find(s => s.id === settings.schedule);
      return { title: schedule?.scene?.name || title, value: t(schedule ? schedule.enabled ? 'key.enabled' : 'key.paused' : settings.schedule ? 'error.target_missing' : 'error.configure'),
        detail: schedule ? `${schedule.start}–${schedule.end}` : '', tone: schedule?.enabled ? 'active' : !schedule ? 'warning' : 'normal' };
    }
    case 'disk': return { title, value: diskSpace(state.disk?.free_bytes, settings.language),
      detail: Number.isFinite(state.disk?.hours_left) ? `${new Intl.NumberFormat(settings.language, { maximumFractionDigits: 1 }).format(state.disk.hours_left)} h` : '',
      tone: Number.isFinite(state.disk?.hours_left) && state.disk.hours_left < 1 ? 'warning' : 'normal' };
  }
}

// La seconde pression confirme exactement la même commande, dans le même contexte.
export class Confirmation {
  constructor() { this.clear(); }
  clear() { this.pending = null; }
  remember(request, settings, snapshot, now = Date.now()) {
    this.pending = { request, settings: JSON.stringify(settings), generation: snapshot.generation,
      scene: snapshot.state.scene.id, scheduledUntil: snapshot.state.recording.scheduled_until,
      expires: now + 5000 };
  }
  take(request, settings, snapshot, now = Date.now()) {
    const p = this.pending; this.clear();
    return Boolean(p && now < p.expires && p.generation === snapshot.generation && p.scene === snapshot.state?.scene.id &&
      p.scheduledUntil === snapshot.state?.recording.scheduled_until && p.settings === JSON.stringify(settings) &&
      JSON.stringify(p.request) === JSON.stringify(request));
  }
}
