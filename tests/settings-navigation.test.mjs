import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { buildSync } from 'esbuild';
import React from 'react';

const result = buildSync({ entryPoints: ['src/components/settings/navigation.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { settingsPages, settingsGroups, settingsCopy, findSettingsPages } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const source = readFileSync('src/components/ConfigWindow.tsx', 'utf8').replace(/\r\n/g, '\n');
const renderedPages = [...source.slice(source.indexOf('  const renderTabContent =')).matchAll(/^      case '([^']+)':/gm)].map((match) => match[1]);
assert.equal(new Set(settingsPages.map((page) => page.key)).size, settingsPages.length);
assert.deepEqual([...renderedPages].sort(), settingsPages.map((page) => page.key).sort(), 'Every navigation destination must have a rendered page');
for (const key of ['general', 'ai', 'tools', 'memory', 'voice', 'network', 'connections', 'plugins', 'about']) {
  assert.ok(settingsPages.some((page) => page.key === key), `Keep existing URL/event destination ${key}`);
}
for (const copy of Object.values(settingsCopy)) {
  for (const page of settingsPages) {
    assert.ok(copy.pages[page.key][0] && copy.pages[page.key][1]);
    assert.ok(copy.groups[page.group]);
  }
}
assert.equal(settingsGroups.length, 4);
assert.ok(settingsPages.find((page) => page.key === 'memory').searchKeys.includes('config.section_rerank'), 'Index settings after nested service status switches');
assert.ok(settingsPages.find((page) => page.key === 'speech').searchKeys.includes('config.section_tts_cross_lang'), 'Index all speech engines and multilingual output');
const translate = (key) => ({ 'config.field_world_latitude': '纬度', 'config.field_proxy_mode': '代理模式' }[key] ?? '');
assert.equal(findSettingsPages('   ', settingsCopy.zh, translate).length, settingsPages.length);
assert.deepEqual(findSettingsPages('纬度', settingsCopy.zh, translate).map((page) => page.key), ['world']);
assert.deepEqual(findSettingsPages('代理', settingsCopy.zh, translate).map((page) => page.key), ['network']);
assert.deepEqual(findSettingsPages('SPEECH output', settingsCopy.en, translate).map((page) => page.key), ['speech']);
assert.deepEqual(findSettingsPages('Token', settingsCopy.zh, translate).map((page) => page.key), ['usage']);
assert.equal(findSettingsPages('no-such-setting-xyz', settingsCopy.zh, translate).length, 0);
assert.ok(source.includes("onGoSearch={() => {\n          setActiveTab('search')"), 'Setup guide opens the new search destination');
assert.ok(source.includes("onGoVoice={() => {\n          setActiveTab('speech')"), 'Setup guide opens speech output');
const sectionsBundle = buildSync({ entryPoints: ['src/components/settings/SettingsSections.tsx'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { default: SettingsSections } = await import(`data:text/javascript;base64,${Buffer.from(sectionsBundle.outputFiles[0].text).toString('base64')}`);
const fragment = (value) => React.createElement(React.Fragment, null, React.createElement('input', { key: 'field', value }));
const grouped = SettingsSections({ contentsLabel: 'Contents', children: React.createElement(React.Fragment, null, fragment('one'), fragment('two')) });
const fields = grouped.props.children[1].props.children[0].props.children;
assert.equal(fields.length, 2);
assert.equal(new Set(fields.map((field) => field.key)).size, 2, 'Flattened fragments must preserve distinct parent key paths');
console.log('Settings navigation: destinations, legacy links, languages and search checks passed.');
