// Run with: node soulkit/tests/settings-ui.cjs
// Exercises the actual navigation/save/close flow without a WebView or dependencies.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const ui = path.resolve(__dirname, '../ui');
const html = fs.readFileSync(path.join(ui, 'index.html'), 'utf8');
const css = fs.readFileSync(path.join(ui, 'styles.css'), 'utf8');
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
assert.equal(new Set(ids).size, ids.length, 'HTML IDs must be unique');
assert(css.match(/\.settings-heading\s*\{[^}]*position:\s*sticky;[^}]*top:\s*0;/), 'Settings header must remain sticky');
class Node {
  constructor() {
    this.textContent = ''; this.children = []; this.style = {}; this.dataset = {};
    this.hidden = false; this.open = false; this.value = ''; this.checked = false;
    this.classList = {toggle() {}, add() {}, remove() {}};
  }
  append(...items) { this.children.push(...items); }
  replaceChildren(...items) { this.children = items; }
  querySelector() { return this.child ||= new Node(); }
  querySelectorAll() { return []; }
  addEventListener() {}
  setAttribute() {}
  remove() {}
  focus() {}
  showModal() { assert(!this.open, 'Duplicate modal'); this.open = true; }
  close() { this.open = false; }
  get nextElementSibling() { return this.sibling ||= new Node(); }
}
const nodes = new Map(ids.map(id => [id, new Node()]));
const locales = ['en', 'fr'].map(value => Object.assign(new Node(), {value, checked: true}));
const environment = {cache: {found: false}, output_storage: {path: 'C:/output', available_bytes: 1024}, scratch_storage: {path: 'C:/work', available_bytes: 1024}};
const initial = {workspace: 'C:/work', output: 'C:/output', soulkit: 'C:/soulkit.exe', cache_path: null, dll_directory: null, locales: 'en,fr', keep_intermediates: false};
let failSave = false, savedRequests = [];
const context = {
  document: {
    documentElement: {dataset: {theme: 'dark'}},
    getElementById(id) { assert(nodes.has(id), 'Missing DOM ID ' + id); return nodes.get(id); },
    querySelectorAll(selector) {
      if (selector === '#locale-options input') return locales;
      if (selector === '#locale-options input:checked') return locales.filter(node => node.checked);
      return [];
    },
    createElement: () => new Node(), createTextNode: text => ({textContent: text})
  },
  window: {__TAURI__: {core: {invoke: async (command, args) => {
    if (command === 'save_settings') {
      savedRequests.push(args.settings);
      if (failSave) throw new Error('Cannot write settings');
      return {settings: {...args.settings}, version: '0.1.0', game_found: false};
    }
    if (command === 'inspect_run_environment') return environment;
    if (command === 'inspect_runtime_libraries') return {directory: 'C:/app', files: []};
    if (command === 'plan_run') return {jobs: [], modules: [], dependencies: []};
    if (command === 'library_stats') return {files: 0};
    if (command === 'list_directory') return [];
    throw new Error('Unexpected command ' + command);
  }}}},
  setTimeout: () => 0, clearTimeout() {}, requestAnimationFrame: work => work(), console
};
vm.createContext(context);
const source = fs.readFileSync(path.join(ui, 'app.js'), 'utf8').replace(/initialize\(\);\s*$/, '');
for (const match of source.matchAll(/\$\('([^']+)'\)/g)) {
  if (!match[1].endsWith('-')) assert(nodes.has(match[1]), 'Missing literal DOM ID ' + match[1]);
}
vm.runInContext(source + '\nthis.api = {state, fillSettings, hasUnsavedSettings, finishSettingsChoice, navigate, renderRun, guardWindowClose};', context);
const api = context.api;
const tick = () => new Promise(resolve => setImmediate(resolve));
function edit() {
  api.state.page = 'settings'; api.state.settings = {...initial}; api.state.run = {status: 'idle'};
  api.fillSettings();
  nodes.get('setting-output').value = 'D:/new output';
  assert.equal(api.hasUnsavedSettings(), true);
}
async function choose(promise, choice) {
  await tick(); assert.equal(nodes.get('unsaved-settings-dialog').open, true);
  api.finishSettingsChoice(choice); return promise;
}
(async () => {
  api.state.settings = {...initial}; api.state.page = 'settings'; api.fillSettings();
  assert.equal(api.hasUnsavedSettings(), false);
  context.document.documentElement.dataset.theme = 'light';
  assert.equal(api.hasUnsavedSettings(), false, 'Theme saves immediately and must not dirty extraction settings');
  edit();
  api.renderRun();
  assert.equal(nodes.get('setting-output').value, 'D:/new output', 'Status updates must preserve drafts');
  assert.equal(await api.navigate('settings'), true);
  assert.equal(nodes.get('setting-output').value, 'D:/new output', 'Re-entering Settings must preserve drafts');
  assert.equal(await choose(api.navigate('library'), 'keep'), false);
  assert.equal(api.state.page, 'settings');
  assert.equal(nodes.get('setting-output').value, 'D:/new output');

  edit();
  const first = api.navigate('library'); await tick();
  assert.equal(await api.navigate('activity'), false, 'A second navigation must not create another prompt');
  api.finishSettingsChoice('keep'); await first;

  edit();
  assert.equal(await choose(api.navigate('extract'), 'discard'), true);
  assert.equal(api.state.page, 'extract');
  assert.equal(nodes.get('setting-output').value, initial.output);
  assert.equal(context.document.documentElement.dataset.theme, 'light', 'Discard must preserve the immediately saved theme');

  edit();
  assert.equal(await choose(api.navigate('library'), 'save'), true);
  assert.equal(savedRequests.at(-1).output, 'D:/new output');
  assert.equal(api.state.page, 'library');
  assert.equal(api.hasUnsavedSettings(), false);

  edit(); failSave = true;
  assert.equal(await choose(api.navigate('activity'), 'save'), false);
  assert.equal(api.state.page, 'settings');
  assert.equal(nodes.get('setting-output').value, 'D:/new output');
  assert.equal(api.hasUnsavedSettings(), true);
  assert.equal(nodes.get('save-settings').disabled, false);
  failSave = false;

  edit(); locales.forEach(node => { node.checked = false; });
  const savesBefore = savedRequests.length;
  assert.equal(await choose(api.navigate('extract'), 'save'), false);
  assert.equal(savedRequests.length, savesBefore, 'Invalid drafts must not be saved or discarded');
  assert.equal(api.state.page, 'settings');

  edit(); let prevented = 0, destroyed = 0;
  const event = {preventDefault() { prevented += 1; }};
  const window = {async destroy() { destroyed += 1; }};
  await choose(api.guardWindowClose(event, window), 'keep');
  assert.equal(prevented, 1); assert.equal(destroyed, 0);
  assert.equal(api.hasUnsavedSettings(), true);
  await choose(api.guardWindowClose(event, window), 'discard');
  assert.equal(destroyed, 1);
  assert.equal(api.hasUnsavedSettings(), false);
  console.log('Settings UI tests passed: sticky header, dirty tracking, navigation keep/discard/save, failed/invalid save, draft preservation, theme exclusion, repeated navigation, and native close guard.');
})().catch(error => { console.error(error); process.exitCode = 1; });
