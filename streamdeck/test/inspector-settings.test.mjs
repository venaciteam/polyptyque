import test from 'node:test';
import assert from 'node:assert/strict';
import { InspectorSettings } from '../src/inspector-settings.mjs';

test('un ancien état ne fait pas revenir le choix de scène avant ou après confirmation', () => {
  const state = new InspectorSettings({ scene: 'a', language: 'fr' });
  const saved = state.change('scene', 'b', 'revision-b', 100);
  assert.equal(state.receive({ scene: 'a', language: 'fr' }), false);
  assert.equal(state.current.scene, 'b'); assert.ok(state.pending);
  assert.equal(state.receive(saved), true); assert.equal(state.pending, null);
  assert.equal(state.receive({ scene: 'a', language: 'fr' }), false); assert.equal(state.current.scene, 'b');
  // Une réouverture retrouve le choix réellement conservé par Stream Deck.
  assert.equal(new InspectorSettings(saved).current.scene, 'b');
});
test('deux changements rapides puis retour au choix initial ignorent les accusés périmés', () => {
  const state = new InspectorSettings({ scene: 'a' });
  const first = state.change('scene', 'b', 'one');
  const second = state.change('mode', 'on', 'two');
  const last = state.change('scene', 'a', 'three');
  assert.equal(state.receive(first), false); assert.equal(state.receive(second), false);
  assert.equal(state.current.scene, 'a'); assert.equal(state.current.mode, 'on');
  assert.equal(state.receive(last), true); assert.equal(state.receive(second), false);
  assert.equal(state.current.scene, 'a'); assert.equal(state.current.mode, 'on');
});
