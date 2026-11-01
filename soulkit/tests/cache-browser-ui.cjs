// Run with: node soulkit/tests/cache-browser-ui.cjs
// Executes the real frontend against an in-memory, paginated native IPC contract.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const ui = path.resolve(__dirname, '../ui');
const html = fs.readFileSync(path.join(ui, 'index.html'), 'utf8');
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
assert.equal(ids.length, new Set(ids).size, 'Unique DOM IDs');
const cacheSource = fs.readFileSync(path.join(ui, 'cache-browser.js'), 'utf8');
for (const match of cacheSource.matchAll(/\$\('([^']+)'\)/g)) assert(ids.includes(match[1]), 'Missing cache DOM ID ' + match[1]);
class Node {
  constructor() {
    this.textContent = ''; this.children = []; this.style = {}; this.dataset = {}; this.attributes = {};
    this.value = ''; this.checked = false; this.hidden = false; this.open = false; this.events = {};
    this.classes = new Set(); this.classList = {toggle: (name, value) => value ? this.classes.add(name) : this.classes.delete(name), add: name => this.classes.add(name), remove: name => this.classes.delete(name)};
  }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  addEventListener(name, handler) { this.events[name] = handler; }
  setAttribute(name, value) { this.attributes[name] = value; }
  removeAttribute(name) { delete this.attributes[name]; }
  querySelector() { return this.child ||= new Node(); }
  querySelectorAll() { return []; }
  focus() {}
  showModal() { this.open = true; }
  close() { this.open = false; }
  pause() {}
  load() {}
  remove() {}
  get nextElementSibling() { return this.sibling ||= new Node(); }
}
const nodes = new Map(ids.map(id => [id, new Node()]));
nodes.get('cache-image-format').value = 'png';
const locales = [Object.assign(new Node(), {value: 'en', checked: true})];
const assets = [
  ...Array.from({length: 205}, (_, index) => ({id: 'image:' + index, path: 'images/Artwork/match-' + String(index).padStart(3, '0') + '.png', kind: 'images'})),
  {id: 'image:tile', path: 'images/WorldTiles/Tile.png', kind: 'images'},
  {id: 'audio:en', path: 'audio/Sound.en/Voice/Hello.wav', kind: 'audio'},
  {id: 'audio:fr', path: 'audio/Sound.fr/Voice/Hello.wav', kind: 'audio'},
  {id: 'model:barrel', path: 'models/Props/Barrel.fbx', kind: 'models'},
  {id: 'model:arch', path: 'models/Scenery/Arch.fbx', kind: 'models'}
].map(asset => ({...asset, name: asset.path.split('/').pop(), is_dir: false, count: 1, compressed_bytes: 32, decompressed_bytes: 64, cache_sets: []}));
const selected = new Set(), calls = [], clearedTimers = new Set();
let timerId = 0;
let generation = 7, rejectSelection = false, deferredBrowse = null, deferredPreview = null;
function matching(scope) {
  return assets.filter(asset => (!scope.folder || asset.path.startsWith(scope.folder + '/')) && (!scope.query || asset.path.toLowerCase().includes(scope.query.toLowerCase())) && (!scope.kind || scope.kind === 'all' || asset.kind === scope.kind));
}
function summary() {
  const values = assets.filter(asset => selected.has(asset.id));
  return {count: values.length, images: values.filter(asset => asset.kind === 'images').length, audio: values.filter(asset => asset.kind === 'audio').length, models: values.filter(asset => asset.kind === 'models').length, compressed_bytes: values.length * 32, decompressed_bytes: values.length * 64};
}
function browse(request) {
  if (request.refresh) { generation += 1; selected.clear(); }
  if (request.generation !== null && request.generation !== undefined && request.generation !== generation) throw new Error('Cache catalog changed. Rescan the cache.');
  const found = matching(request), entries = [];
  if (request.query) entries.push(...found.map(asset => ({...asset, selected: selected.has(asset.id), partial: false})));
  else {
    const folders = new Map();
    for (const asset of found) {
      const rest = asset.path.slice(request.folder ? request.folder.length + 1 : 0), parts = rest.split('/');
      if (parts.length === 1) entries.push({...asset, selected: selected.has(asset.id), partial: false});
      else {
        const folderPath = (request.folder ? request.folder + '/' : '') + parts[0];
        const group = folders.get(folderPath) || []; group.push(asset); folders.set(folderPath, group);
      }
    }
    for (const [folderPath, values] of folders) {
      const selectedCount = values.filter(asset => selected.has(asset.id)).length;
      entries.push({id: 'folder:' + folderPath, path: folderPath, name: folderPath.split('/').pop(), kind: 'folder', is_dir: true, count: values.length, compressed_bytes: values.length * 32, selected: selectedCount === values.length, partial: selectedCount > 0 && selectedCount < values.length});
    }
  }
  entries.sort((a, b) => a.path.localeCompare(b.path));
  return {generation, cache_path: 'C:/game/Cache.Windows', folder: request.folder, query: request.query, kind: request.kind, total: entries.length, offset: request.offset, limit: request.limit, entries: entries.slice(request.offset, request.offset + request.limit), selection: summary(), total_assets: assets.length, matching_assets: found.length};
}
async function ipc(command, args = {}) {
  calls.push({command, args});
  if (command === 'cache_cancel_preview' || command === 'cache_cancel_browse') return;
  if (command === 'cache_browse') {
    const data = browse(args.request);
    if (deferredBrowse) return deferredBrowse(data);
    return data;
  }
  if (command === 'cache_select') {
    if (rejectSelection) throw new Error('Cache catalog changed. Rescan the cache.');
    const request = args.request;
    assert.equal(request.generation, generation);
    let values = request.ids ? assets.filter(asset => request.ids.some(id => id === asset.id || id.startsWith('folder:') && asset.path.startsWith(id.slice(7) + '/'))) : matching(request);
    if (request.action === 'clear' || request.action === 'replace') selected.clear();
    if (request.action !== 'clear') for (const asset of values) request.action === 'remove' ? selected.delete(asset.id) : selected.add(asset.id);
    return summary();
  }
  if (command === 'cache_preview') {
    if (deferredPreview) return deferredPreview(args);
    if (assets.find(asset => asset.id === args.id).kind === 'models') return {path: null, kind: 'model', notice: 'Models have no in-app preview. Extract to get a .glb you can open in Blender or any glTF viewer.'};
    return {path: 'C:/app-cache/one.png', kind: 'image', notice: null};
  }
  if (command === 'cache_extract') return {jobs: [{id: 'cache', label: 'Selected cache assets'}], modules: ['images', 'audio', 'models'].filter(kind => summary()[kind]), dependencies: []};
  if (command === 'inspect_run_environment') return {cache: {found: true}, output_storage: null, scratch_storage: null};
  if (command === 'inspect_runtime_libraries') return {directory: 'C:/app', files: []};
  throw new Error('Unexpected IPC ' + command);
}
const context = {
  document: {
    documentElement: {dataset: {theme: 'dark'}},
    getElementById(id) { assert(nodes.has(id), 'Missing runtime ID ' + id); return nodes.get(id); },
    querySelectorAll(selector) { return selector === '#locale-options input' || selector === '#locale-options input:checked' ? locales : []; },
    createElement: () => new Node(), createTextNode: text => ({textContent: text})
  },
  window: {__TAURI__: {core: {invoke: ipc, convertFileSrc: value => 'asset://' + value}}},
  localStorage: {}, navigator: {}, console, setTimeout: () => ++timerId, clearTimeout(id) { clearedTimers.add(id); }, requestAnimationFrame: work => work()
};
vm.createContext(context);
const appSource = fs.readFileSync(path.join(ui, 'app.js'), 'utf8').replace(/initialize\(\);\s*$/, '');
vm.runInContext(cacheSource + '\n' + appSource + '\nthis.api = {state,cacheBrowser,wireCacheEvents,loadCacheBrowser,cancelCacheLoading,selectCacheAssets,previewCacheAsset,closeCachePreview,extractCacheSelection,renderCacheControls,renderCacheRun,renderRun,navigate,fillSettings,finishSettingsChoice};', context);
const api = context.api, tick = () => new Promise(resolve => setImmediate(resolve));
const countCalls = command => calls.filter(call => call.command === command).length;
(async () => {
  api.wireCacheEvents();
  await api.extractCacheSelection(); assert.equal(countCalls('cache_extract'), 0, 'Empty selection never starts all assets');
  await api.loadCacheBrowser();
  assert.equal(countCalls('cache_preview'), 0, 'Browsing only reads catalog metadata');
  assert.equal(api.cacheBrowser.catalog.total_assets, 210);
  assert.equal(nodes.get('cache-extract').disabled, true);

  api.cacheBrowser.folder = 'images/Artwork'; await api.loadCacheBrowser();
  assert.equal(nodes.get('cache-rows').children.length, 100, 'One bounded page is rendered');
  assert.equal(nodes.get('cache-next').disabled, false);
  await api.selectCacheAssets('add', {ids: ['image:0']});
  assert.equal(selected.size, 1); assert.equal(api.cacheBrowser.selection.count, 1, 'One exact checkbox selects one asset');
  api.cacheBrowser.offset = 100; await api.loadCacheBrowser();
  assert.equal(api.cacheBrowser.selection.count, 1, 'Selection survives pagination');
  assert.equal(api.cacheBrowser.catalog.entries.some(entry => entry.selected), false);
  assert.equal(nodes.get('cache-page-count').textContent, '101–200 of 205');
  await api.selectCacheAssets('add', {ids: ['image:100']}); assert.equal(selected.size, 2);

  api.cacheBrowser.query = 'match-'; api.cacheBrowser.offset = 0; await api.loadCacheBrowser();
  await api.selectCacheAssets('add');
  assert.equal(selected.size, 205, 'Select all results includes every page, constrained to folder and query');
  assert.equal(selected.has('image:tile'), false); assert.equal(selected.has('audio:en'), false);
  await api.selectCacheAssets('clear', {}); assert.equal(selected.size, 0);
  await api.selectCacheAssets('add', {folder: '', query: '', kind: 'all'});
  assert.equal(selected.size, 210, 'Explicit whole-game selection includes images, each audio family, and static models');
  assert.equal(api.cacheBrowser.selection.audio, 2);
  assert.equal(api.cacheBrowser.selection.models, 2);

  const beforePreviewCount = selected.size;
  await api.previewCacheAsset(assets[0]);
  assert.equal(countCalls('cache_preview'), 1); assert.equal(selected.size, beforePreviewCount, 'Previewing does not alter extraction selection');
  assert.equal(nodes.get('cache-preview-stage').children[0].src, 'asset://C:/app-cache/one.png');
  const firstImage = nodes.get('cache-preview-stage').children[0];
  let finishFirst;
  deferredPreview = args => args.id === assets[1].id ? new Promise(resolve => { finishFirst = resolve; }) : Promise.resolve({path: 'C:/app-cache/new.png', kind: 'image'});
  const oldPreview = api.previewCacheAsset(assets[1]); await tick();
  await api.previewCacheAsset(assets[2]); finishFirst({path: 'C:/app-cache/stale.png', kind: 'image'}); await oldPreview;
  assert.equal(nodes.get('cache-preview-stage').children[0].src, 'asset://C:/app-cache/new.png', 'Late previews cannot replace the newest asset');
  firstImage.events.error();
  assert.equal(nodes.get('cache-preview-stage').children[0].src, 'asset://C:/app-cache/new.png', 'An earlier image load error must not overwrite the current preview');
  deferredPreview = null;

  let finishBrowse;
  deferredBrowse = data => new Promise(resolve => { finishBrowse = () => resolve(data); });
  nodes.get('cache-search').value = 'match-'; nodes.get('cache-search').events.input();
  const pendingSearch = api.cacheBrowser.searchTimer;
  const oldBrowse = api.loadCacheBrowser(true); await tick();
  assert(clearedTimers.has(pendingSearch), 'Explicit Rescan cancels the queued search that could adopt an old catalog generation');
  assert.equal(nodes.get('cache-kind').disabled, true); assert.equal(nodes.get('cache-search').disabled, true, 'Filters cannot supersede a running catalog scan');
  api.cancelCacheLoading(); finishBrowse(); await oldBrowse;
  assert.equal(api.cacheBrowser.catalog, null, 'Cancelled loading ignores late catalog results');
  assert.equal(countCalls('cache_cancel_browse'), 1);
  deferredBrowse = null; await api.loadCacheBrowser(true);

  rejectSelection = true; await api.selectCacheAssets('remove', {ids: ['image:0']});
  assert.equal(api.cacheBrowser.valid, false); assert.equal(nodes.get('cache-extract').disabled, true, 'Rejected stale mutation disables extraction');
  const extractionBefore = countCalls('cache_extract'); await api.extractCacheSelection(); assert.equal(countCalls('cache_extract'), extractionBefore);
  rejectSelection = false; await api.loadCacheBrowser(true);
  assert.equal(api.cacheBrowser.selection.count, 0, 'Rescan clears the prior generation selection');
  api.cacheBrowser.folder = 'models/Props'; api.cacheBrowser.kind = 'models'; api.cacheBrowser.query = ''; await api.loadCacheBrowser();
  assert.equal(api.cacheBrowser.catalog.total, 1, 'Model filter and folder browsing exclude other media');
  assert.equal(nodes.get('cache-rows').children[0].children[1].children[1].children[1].textContent, 'Model');
  assert(nodes.get('cache-breadcrumbs').children.some(node => node.textContent === 'Models'));
  await api.selectCacheAssets('add', {ids: ['model:barrel']});
  assert.equal(api.cacheBrowser.selection.models, 1);
  assert(nodes.get('cache-selection-detail').textContent.includes('1 models'));
  assert.equal(nodes.get('cache-image-format').disabled, true, 'Image format is disabled for a model-only selection');
  await api.previewCacheAsset(assets.find(asset => asset.id === 'model:barrel'));
  assert.equal(nodes.get('cache-preview-kind').textContent, 'MODEL');
  assert(nodes.get('cache-preview-stage').children[0].textContent.includes('.glb'), 'Models explain how to view exported GLB files');
  await api.selectCacheAssets('clear', {});
  await api.selectCacheAssets('add', {ids: ['image:tile', 'audio:fr', 'model:barrel']});
  nodes.get('cache-image-format').value = 'dds'; await api.extractCacheSelection();
  const extraction = calls.findLast(call => call.command === 'cache_extract');
  assert.equal(extraction.args.imageFormat, 'dds'); assert.equal(extraction.args.generation, generation);
  assert.equal('all' in extraction.args, false, 'Extraction is explicitly selected, never implicitly all');
  assert.deepEqual(Array.from(api.state.runPlan.modules), ['images', 'audio', 'models']);
  assert.equal(nodes.get('cache-run-status').hidden, false);
  api.state.plan = {jobs: [{id: 'ordinary', label: 'Unrelated normal tool run'}], modules: [], dependencies: []};
  api.state.run = {status: 'completed', job_index: 1, total_jobs: 1, progress: 1}; api.renderRun();
  assert.equal(nodes.get('job-list').children[0].children[0].textContent, 'Selected cache assets', 'Refreshing the next Extract plan must preserve the Activity run snapshot');

  api.state.run = {status: 'idle'}; api.state.page = 'settings';
  api.state.settings = {workspace: 'C:/work', output: 'C:/output', soulkit: 'C:/app.exe', cache_path: null, dll_directory: null, locales: 'en', keep_intermediates: false};
  api.fillSettings(); nodes.get('setting-cache').value = 'D:/unsaved';
  const navigation = api.navigate('cache'); await tick(); assert.equal(nodes.get('unsaved-settings-dialog').open, true);
  api.finishSettingsChoice('keep'); assert.equal(await navigation, false); assert.equal(api.state.page, 'settings', 'Cache navigation honors unsaved Settings guard');
  assert.equal(nodes.get('setting-cache').value, 'D:/unsaved');
  console.log('Cache browser UI passed: bounded pages, exact and recursive selection, whole game, model filter/counts/GLB preview notice, persistent counts, lazy and stale image previews, cancel, generation rejection/rescan, selected extraction formats, and Settings navigation guard.');
})().catch(error => { console.error(error); process.exitCode = 1; });
