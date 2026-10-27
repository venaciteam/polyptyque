import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { I18nProvider } from '@elgato/utils/i18n';
import { fr, en } from '../src/locales.mjs';

function keys(object, prefix = '') {
  return Object.entries(object).flatMap(([key, value]) => typeof value === 'string' ? [`${prefix}${key}`] : keys(value, `${prefix}${key}.`));
}
test('chaque traduction est résolue par le fournisseur SDK réel et ses fichiers distribués', () => {
  assert.deepEqual(keys(fr).sort(), keys(en).sort());
  const provider = new I18nProvider('fr', lang => JSON.parse(readFileSync(`city.vena.polyptyque.sdPlugin/${lang}.json`)).Localization);
  for (const lang of ['fr', 'en']) for (const key of keys(fr)) {
    assert.notEqual(provider.t(key, lang), key, `${lang}: ${key}`);
    assert.ok(provider.t(key, lang).trim());
  }
});
