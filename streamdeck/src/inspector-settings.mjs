// Un état ancien du plugin ne doit pas effacer une sélection qui vient d'être faite.
// La révision est conservée par Stream Deck avec les autres réglages de la touche.
export class InspectorSettings {
  constructor(settings = {}) {
    this.current = { ...settings }; this.pending = null; this.obsolete = new Set(); this.edited = false;
  }
  change(key, value, revision, now = Date.now()) {
    this.obsolete.add(this.current._polyptyqueRevision ?? null);
    this.current = { ...this.current, [key]: value, _polyptyqueRevision: revision };
    this.pending = { revision, since: now }; this.edited = true;
    return this.current;
  }
  receive(settings) {
    const revision = settings._polyptyqueRevision ?? null;
    if (this.pending && revision !== this.pending.revision) return false;
    if (this.obsolete.has(revision)) return false;
    if (revision !== (this.current._polyptyqueRevision ?? null)) this.obsolete.add(this.current._polyptyqueRevision ?? null);
    this.current = { ...settings }; this.pending = null;
    return true;
  }
}
