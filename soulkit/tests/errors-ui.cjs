const fs = require('fs');
const path = require('path');
const vm = require('vm');
const assert = require('assert/strict');
const ui = path.resolve(__dirname, '../ui');
const html = fs.readFileSync(path.join(ui, 'index.html'), 'utf8');
const source = fs.readFileSync(path.join(ui, 'app.js'), 'utf8');
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
assert.equal(new Set(ids).size, ids.length, 'Duplicate HTML IDs');
for (const match of source.matchAll(/\$\('([^']+)'\)/g)) {
  if (match[1].endsWith('-')) continue;
  assert(ids.includes(match[1]), 'Missing literal DOM ID ' + match[1]);
}
class Node {
  constructor() { this.textContent = ''; this.children = []; this.style = {}; this.dataset = {}; this.hidden = false; this.open = false; this.value = ''; this.checked = true; this.attributes = {}; this.events = {}; this.classes = new Set(); this.classList = {toggle: (name, value) => { value ? this.classes.add(name) : this.classes.delete(name); }, add: name => this.classes.add(name)}; }
  append(...items) { this.children.push(...items); }
  replaceChildren(...items) { this.children = items; }
  querySelector() { return this.child ||= new Node(); }
  querySelectorAll() { return []; }
  addEventListener(name, handler) { this.events[name] = handler; }
  setAttribute(name, value) { this.attributes[name] = value; }
  remove() {}
  get nextElementSibling() { return this.sibling ||= new Node(); }
}
const nodes = new Map(ids.map(id => [id, new Node()]));
const context = {document: {getElementById: id => { assert(nodes.has(id), 'Missing runtime ID ' + id); return nodes.get(id); }, querySelectorAll: () => [], createElement: () => new Node(), createTextNode: text => ({textContent: text})}, window: {__TAURI__: {core: {invoke: async () => { throw context.rejection; }, convertFileSrc: value => 'asset://' + value}}}, navigator: {}, localStorage: {}, setTimeout: () => 0, clearTimeout: () => {}, requestAnimationFrame: work => work(), console};
vm.createContext(context);
vm.runInContext(source.replace(/initialize\(\);\s*$/, '') + '\nthis.api = {state,TOOLS,applyPreset,renderTools,fileCard,selectFile,runFailure,renderFailure,renderRun,renderLog,runReport,renderRunEnvironment,startRun,failureAction};', context);
const api = context.api;
assert.deepEqual(Array.from(api.TOOLS, tool => tool.id), ['images','audio','models','languages','maps','items','calendars']);
assert(!html.includes('data-folder="scripts"'), 'Public collections must not advertise script extraction');
api.renderTools();
assert.equal(nodes.get('tool-grid').children.length, 7, 'The public Extract page adds models to its six supported tools');
assert.deepEqual(Array.from(api.TOOLS.find(tool => tool.id === 'models').tags), ['GLB']);
api.applyPreset('media');
assert.deepEqual(Array.from(api.state.selected), ['images','audio','models']);
api.applyPreset('data');
assert.deepEqual(Array.from(api.state.selected), ['languages','maps','items','calendars']);
api.applyPreset('all');
assert.equal(api.state.selected.size, 7);
assert(!api.state.selected.has('scripts') && !api.state.selected.has('version'), 'Presets must not request removed worker commands');
const imageEntry = {name: 'Artwork.png', kind: 'image', is_dir: false, relative_path: 'images/Artwork.png', absolute_path: 'C:/output/images/Artwork.png', size: 1024};
const imageCard = api.fileCard(imageEntry), thumbnail = imageCard.children[0].children[0];
assert.equal(thumbnail.src, 'asset://C:/output/images/Artwork.png', 'File-library images keep real asset thumbnails');
assert.equal(thumbnail.loading, 'lazy');
thumbnail.events.error();
assert(imageCard.children[0].innerHTML.includes('<svg'), 'An unsupported thumbnail falls back to an image icon');
const cacheFailure = {kind: 'cache_missing',title: 'Game cache not found',message: 'Soulframe cache was not found.',suggestion: 'Choose Cache.Windows in Settings.',action: 'open_cache_settings',details: ['Checked C:/Games/Soulframe'],exit_code: 1};
api.state.plan = {jobs: [{id: 'extract',label: 'Extract selected modules'}],modules: ['images','audio'],dependencies: []};
api.state.run = {status: 'failed',job_index: 1,total_jobs: 1,progress: 0.5,error: 'generic code 1',failure: cacheFailure};
api.renderRun();
assert.equal(nodes.get('activity-title').textContent, cacheFailure.title);
assert.equal(nodes.get('activity-description').textContent, cacheFailure.message);
assert.equal(nodes.get('activity-progress-value').textContent, '');
assert.equal(nodes.get('extract-failure').hidden, false);
assert.equal(nodes.get('activity-failure-action').textContent, 'Choose game cache');
assert(nodes.get('activity-failure-details-text').textContent.includes('Worker exit code: 1'));
api.state.boot = {version: '0.1.0'};
api.state.logs = ['[!] images extract failed: Soulframe cache not found'];
assert(api.runReport().includes(cacheFailure.suggestion));
assert(api.runReport().includes(api.state.logs[0]));
const full = {...cacheFailure,kind: 'disk_full',title: 'Not enough disk space',action: 'open_output_settings'};
api.state.run.failure = full;
api.renderRun();
assert.equal(nodes.get('activity-title').textContent, 'Not enough disk space');
assert.equal(nodes.get('activity-failure-action').textContent, 'Change folders');
api.state.run = {status: 'completed',progress: 1,job_index: 1,total_jobs: 1,label: 'Extraction complete'};
api.renderRun();
assert.equal(nodes.get('extract-failure').hidden, true);
assert.equal(nodes.get('activity-failure-action').hidden, true);
assert.equal(nodes.get('activity-title').textContent, 'Extraction complete');
api.state.run = {status: 'cancelled',progress: 0.5,label: 'Extraction cancelled'};
api.renderRun();
assert.equal(nodes.get('extract-failure').hidden, true);
api.renderRunEnvironment({cache: {found: false,error: 'No installed game cache was found.'},output_storage: {path:'D:/output',available_bytes: 0,total_bytes: 1024},scratch_storage: {path: 'C:/.soulkit-work',available_bytes: null,total_bytes: null}});
assert.equal(nodes.get('output-storage-free').textContent, '0 B free');
assert.equal(nodes.get('scratch-storage-free').textContent, 'Unavailable');
assert(nodes.get('output-storage-free').classes.has('no-space'));
(async () => {
  await api.selectFile(imageEntry, imageCard);
  assert.equal(nodes.get('preview-stage').children[0].src, thumbnail.src, 'Selecting a library image opens its full image preview');
  assert.equal(nodes.get('preview-stage').children[0].alt, imageEntry.name);
  api.state.run = {status:'idle'};
  api.state.logs = ['old'];
  context.rejection = cacheFailure;
  await api.startRun();
  assert.equal(api.state.run.failure.title, cacheFailure.title);
  assert.equal(api.state.run.job_index, 0);
  assert.equal(nodes.get('job-list').children.length, 0);
  assert(nodes.get('run-log').textContent.startsWith('The run did not start.'));
  console.log('UI smoke passed: seven public tools and allowed presets, image thumbnails and previews, root-cause banner, fix actions, technical details, copy report, preflight rejection, success/cancel, storage zero/unknown, ' + ids.length + ' DOM IDs.');
})().catch(error => { console.error(error); process.exitCode = 1; });
