'use strict';

const ICONS = {
  layers: '<path d="m12 3 9 5-9 5-9-5Z"/><path d="m3 12 9 5 9-5M3 16l9 5 9-5"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z"/>',
  'folder-open': '<path d="M3 9V5h7l2 2h8v3"/><path d="m3 10 2 10h15l2-10Z"/>',
  activity: '<path d="M2 12h5l3-8 4 16 3-8h5"/>',
  settings: '<path d="m9 3-.5 3-2 1-3-.8-2 3 2.4 2v2L1.5 15l2 3 3-.8 2 1L9 21h6l.5-2.8 2-1 3 .8 2-3-2.4-1.8v-2l2.4-2-2-3-3 .8-2-1L15 3Z"/><circle cx="12" cy="12" r="3"/>',
  harddrive: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M3 14h18M7 17h.01M11 17h.01"/>',
  'arrow-right': '<path d="M4 12h16m-6-6 6 6-6 6"/>',
  'arrow-left': '<path d="M20 12H4m6-6-6 6 6 6"/>',
  'chevron-right': '<path d="m9 6 6 6-6 6"/>',
  image: '<rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8" cy="8" r="1.5"/><path d="m3 17 5-5 4 4 4-6 5 7"/>',
  headphones: '<path d="M4 14v-3a8 8 0 0 1 16 0v3"/><rect x="3" y="12" width="4" height="8" rx="2"/><rect x="17" y="12" width="4" height="8" rx="2"/>',
  map: '<path d="m3 6 6-3 6 3 6-3v15l-6 3-6-3-6 3ZM9 3v15m6-12v15"/>',
  languages: '<path d="M3 5h12M9 3v2M5 5c0 5 3 9 8 11M13 5c0 5-4 10-10 13m11 3 4-11 4 11m-7-3h6"/>',
  code: '<path d="m8 6-6 6 6 6m8-12 6 6-6 6m-3-15-2 18"/>',
  shield: '<path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6Z"/><path d="m8 12 3 3 5-6"/>',
  sparkles: '<path d="m12 3 2.4 6.6L21 12l-6.6 2.4L12 21l-2.4-6.6L3 12l6.6-2.4ZM21 2v4m-2-2h4"/>',
  play: '<path d="m8 4 12 8-12 8Z"/>',
  square: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10h.01"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
  refresh: '<path d="M20 7v5h-5M4 17v-5h5"/><path d="M5 8a8 8 0 0 1 13-3l2 3M4 16l2 3a8 8 0 0 0 13-3"/>',
  external: '<path d="M13 3h8v8m0-8L10 14M9 5H3v16h16v-6"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  grid: '<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>',
  list: '<path d="M9 5h12M9 12h12M9 19h12M3 5h.01M3 12h.01M3 19h.01"/>',
  scan: '<path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5M7 12h10"/>',
  x: '<path d="m6 6 12 12M6 18 18 6"/>',
  copy: '<rect x="8" y="8" width="13" height="13" rx="2"/><path d="M16 8V3H3v13h5"/>',
  sliders: '<path d="M4 5h16M4 12h16M4 19h16"/><circle cx="8" cy="5" r="2" fill="currentColor"/><circle cx="16" cy="12" r="2" fill="currentColor"/><circle cx="10" cy="19" r="2" fill="currentColor"/>',
  calendar: '<rect x="3" y="5" width="18" height="16" rx="3"/><path d="M7 3v4m10-4v4M3 11h18m-14 4h2m6 0h2m-10 3h2"/>',
  package: '<path d="m12 3 9 5v9l-9 5-9-5V8Zm0 10 9-5M3 8l9 5v9M8 5l9 5"/>',
  cube: '<path d="m12 3 8 4.5v9L12 21l-8-4.5v-9Z"/><path d="m4 7.5 8 4.5 8-4.5M12 12v9"/>',
  tag: '<path d="M3 3h8l10 10-8 8L3 11Z"/><circle cx="7.5" cy="7.5" r="1"/>',
  file: '<path d="M5 3h9l5 5v13H5Z"/><path d="M14 3v6h5M8 13h8m-8 4h6"/>',
  music: '<path d="M9 18V5l12-2v13M9 9l12-2"/><ellipse cx="6" cy="18" rx="3" ry="3"/><ellipse cx="18" cy="16" rx="3" ry="3"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  alert: '<path d="m12 3 10 18H2Z"/><path d="M12 9v5m0 3h.01"/>'
};

const TOOLS = [
  {id: 'images', title: 'Textures & images', description: 'Interface artwork, textures, and graphics.', icon: 'image', tone: 'blue', tags: ['PNG', 'DDS'], folder: 'images/'},
  {id: 'audio', title: 'Audio', description: 'Music, voice lines, ambience, and effects.', icon: 'headphones', tone: 'purple', tags: ['OGG', 'WAV'], folder: 'audio/'},
  {id: 'models', title: '3D models', description: 'Static meshes: props, structures, and scenery.', icon: 'cube', tone: 'slate', tags: ['GLB'], folder: 'models/'},
  {id: 'languages', title: 'Languages', description: 'Localized strings for your chosen languages.', icon: 'languages', tone: 'green', tags: ['JSON'], folder: 'Languages/'},
  {id: 'maps', title: 'Maps & dungeons', description: 'World tiles, layouts, and map data.', icon: 'map', tone: 'amber', tags: ['WEBP', 'JSON'], folder: 'maps/'},
  {id: 'items', title: 'Items & loot', description: 'Item catalogues, recipes, and drop tables.', icon: 'package', tone: 'rose', tags: ['JSON'], folder: 'output/'},
  {id: 'calendars', title: 'Daily calendars', description: 'Zenith, weather, and Gods and Ghosts.', icon: 'calendar', tone: 'green', tags: ['JSON'], folder: 'calendars'}
];
const LOCALES = [['en', 'English'], ['fr', 'French'], ['de', 'German'], ['es', 'Spanish'], ['it', 'Italian'], ['pt', 'Portuguese'], ['ru', 'Russian'], ['pl', 'Polish'], ['tr', 'Turkish'], ['ja', 'Japanese'], ['ko', 'Korean'], ['zh', 'Chinese']];
const $ = (id) => document.getElementById(id);
const icon = (name) => '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (ICONS[name] || ICONS.file) + '</svg>';
const state = {
  boot: null, settings: null, page: 'extract', selected: new Set(['images', 'audio', 'languages']),
  plan: null, runPlan: null, planError: '', run: {status: 'idle', progress: 0, job_index: 0, total_jobs: 0},
  logs: [], logTimer: null, planRequest: 0, directory: '', entries: [], search: '', dllRequest: 0, environmentRequest: 0, environment: null,
  view: 'grid', selectedFile: null, libraryRequest: 0, fileRenderRequest: 0, previewRequest: 0, stats: null,
  settingsBaseline: null, settingsSaving: false, settingsExitPending: false
};

function paintIcons(root = document) {
  root.querySelectorAll('i[data-icon]').forEach((node) => { node.innerHTML = icon(node.dataset.icon); });
}
function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}
function isRunning() { return ['running', 'cancelling', 'starting'].includes(state.run.status); }
function tool(id) { return TOOLS.find((entry) => entry.id === id); }
function prettyPath(path) { return String(path || '').replace(/^\\\\\?\\/, '').replace(/\\/g, '/'); }
function basename(path) { return prettyPath(path).replace(/\/+$/, '').split('/').pop() || 'Workspace'; }
function bytes(size) {
  if (!Number.isFinite(Number(size))) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let n = Number(size), index = 0;
  while (n >= 1024 && index < units.length - 1) { n /= 1024; index += 1; }
  return n.toLocaleString(undefined, {maximumFractionDigits: index > 1 ? 1 : 0}) + ' ' + units[index];
}
function errorText(error) { return typeof error === 'string' ? error : (error && error.message) || String(error); }
function runFailure(error) {
  let value = error && error.failure || error;
  if (typeof value === 'string' && value.trim().startsWith('{')) {
    try { value = JSON.parse(value); } catch (_) {}
  }
  if (value && typeof value === 'object' && value.title && value.message) {
    return {...value, details: Array.isArray(value.details) ? value.details : []};
  }
  return {kind: 'other', title: 'Extraction could not finish', message: errorText(value) || 'No error details were reported.', suggestion: 'Review the run output below, then try again.', details: [], action: null, exit_code: null};
}
function failureAction(action) {
  return {
    open_cache_settings: {label: 'Choose game cache', field: 'setting-cache'},
    open_dll_settings: {label: 'Open DLL settings', field: 'setting-dll-directory'},
    open_output_settings: {label: 'Change folders', field: 'setting-output'}
  }[action];
}
async function showFailureSettings(action) {
  const target = failureAction(action);
  if (!target) return;
  if (!await navigate('settings')) return;
  requestAnimationFrame(() => {
    const field = $(target.field);
    field.scrollIntoView({block: 'center', behavior: 'smooth'});
    field.focus({preventScroll: true});
  });
}
function failureDetails(failure) {
  const details = (failure.details || []).map(String);
  if (failure.exit_code !== null && failure.exit_code !== undefined) details.push('Worker exit code: ' + failure.exit_code);
  return details.join('\n');
}
function renderFailure(failure) {
  $('extract-failure').hidden = !failure;
  $('activity-overview').classList.toggle('failed', Boolean(failure));
  $('activity-symbol').innerHTML = icon(failure ? 'alert' : 'activity');
  for (const prefix of ['extract-failure', 'activity-failure']) {
    const suggestion = $(prefix + '-suggestion'), action = $(prefix + '-action');
    const details = $(prefix + '-details'), detailText = $(prefix + '-details-text');
    suggestion.hidden = !failure || !failure.suggestion;
    suggestion.textContent = failure && failure.suggestion || '';
    const target = failure && failureAction(failure.action);
    action.hidden = !target;
    action.textContent = target ? target.label : '';
    action.onclick = target ? () => showFailureSettings(failure.action) : null;
    const technical = failure ? failureDetails(failure) : '';
    details.hidden = !technical;
    detailText.textContent = technical;
  }
  $('extract-failure-title').textContent = failure && failure.title || '';
  $('extract-failure-message').textContent = failure && failure.message || '';
}
function runReport() {
  const header = ['Soulkit ' + (state.boot && state.boot.version || ''), 'Status: ' + (state.run.status || 'idle')];
  const requested = state.runPlan && state.runPlan.requested || Array.from(state.selected);
  const selected = requested.map((id) => tool(id)).filter(Boolean).map((entry) => entry.title);
  if (selected.length) header.push('Selected tools: ' + selected.join(', '));
  if (state.run.status === 'failed') {
    const failure = runFailure(state.run.failure || state.run.error);
    header.push('', failure.title, failure.message);
    if (failure.suggestion) header.push(failure.suggestion);
    const details = failureDetails(failure);
    if (details) header.push('', details);
  }
  if (state.logs.length) header.push('', 'Run output:', ...state.logs);
  return header.join('\n');
}
function toast(message, type = 'success') {
  const node = element('div', 'toast ' + type);
  node.innerHTML = icon(type === 'error' ? 'alert' : 'check');
  node.append(element('span', '', message));
  $('toast-region').append(node);
  setTimeout(() => node.remove(), 6500);
}
async function invoke(name, args) {
  if (!window.__TAURI__) throw new Error('Open Soulkit as a desktop app to access your workspace.');
  return window.__TAURI__.core.invoke(name, args || {});
}
function assetUrl(path) { return window.__TAURI__.core.convertFileSrc(path); }

function renderTools() {
  const grid = $('tool-grid');
  grid.replaceChildren();
  for (const entry of TOOLS) {
    const card = element('label', 'tool-card tone-' + entry.tone);
    card.dataset.tool = entry.id;
    const input = element('input', 'tool-checkbox');
    input.type = 'checkbox';
    input.id = 'tool-' + entry.id;
    input.checked = state.selected.has(entry.id);
    input.setAttribute('aria-label', 'Extract ' + entry.title);
    input.addEventListener('change', () => {
      if (input.checked) state.selected.add(entry.id); else state.selected.delete(entry.id);
      document.querySelectorAll('[data-preset]').forEach((button) => button.classList.toggle('active', button.dataset.preset === 'custom'));
      saveSelection();
      renderSelection();
      refreshPlan();
    });
    card.append(input);
    const main = element('div', 'tool-card-main');
    const glyph = element('span', 'tool-icon');
    glyph.innerHTML = icon(entry.icon);
    const copy = element('div', 'tool-copy');
    copy.append(element('h3', '', entry.title), element('p', '', entry.description));
    main.append(glyph, copy);
    const footer = element('div', 'tool-card-footer');
    const tags = element('span', 'format-tags');
    entry.tags.forEach((tag) => tags.append(element('span', '', tag)));
    footer.append(tags, element('span', 'tool-destination', entry.folder));
    card.append(main, footer);
    grid.append(card);
  }
}
function saveSelection() {
  try { localStorage.setItem('soulkit.selected', JSON.stringify(Array.from(state.selected))); } catch (_) {}
}
function renderSelection() {
  const dependencies = state.plan ? state.plan.dependencies || [] : [];
  const required = new Set(dependencies.map((dependency) => dependency.task));
  document.querySelectorAll('.tool-card').forEach((card) => {
    const checked = state.selected.has(card.dataset.tool);
    const supporting = required.has(card.dataset.tool) && !checked;
    const input = card.querySelector('input');
    input.checked = checked || supporting;
    input.disabled = isRunning() || supporting;
    card.classList.toggle('selected', checked || supporting);
    card.classList.toggle('supporting', supporting);
    card.classList.toggle('disabled', isRunning());
    input.title = supporting ? 'Required by another selected tool' : '';
  });
  $('selection-count').textContent = String(state.selected.size);
  $('selection-count').nextElementSibling.textContent = state.selected.size === 1 ? 'tool selected' : 'tools selected';
  $('selection-badge').textContent = state.selected.size === TOOLS.length ? 'All tools' : 'Custom run';
  const selected = $('selected-tools');
  selected.replaceChildren();
  for (const entry of TOOLS.filter((item) => state.selected.has(item.id))) {
    const row = element('div', 'selected-tool-row');
    row.innerHTML = icon('check');
    row.append(element('span', '', entry.title));
    selected.append(row);
  }
  if (!state.selected.size) selected.append(element('p', 'selection-empty', 'Select the tools you’d like to run.'));
  const note = $('dependency-note');
  note.replaceChildren();
  const includesItems = state.plan && state.plan.modules.includes('items');
  note.hidden = dependencies.length === 0 && !includesItems;
  if (dependencies.length) {
    note.append(element('strong', '', 'Required supporting tools'));
    for (const dependency of dependencies) {
      const entry = tool(dependency.task);
      note.append(element('div', 'dependency-chip', entry ? entry.title : dependency.task));
      note.append(element('p', '', dependency.reason));
    }
  }
  if (includesItems) note.append(element('p', 'tool-run-note', 'Items & loot may open Soulframe if fresh decryption keys are needed.'));
  const enabled = state.selected.size > 0 && !state.planError && state.plan && state.plan.jobs.length > 0 && !isRunning();
  $('run-button').disabled = !enabled;
  $('run-button').querySelector('span').textContent = isRunning() ? 'Extraction in progress' : 'Run selected tools';
  $('plan-status').textContent = isRunning() ? 'Running · See Activity' :
    state.planError || (state.selected.size ? '' : 'Choose at least one tool to begin.');
  document.querySelectorAll('[data-preset],#clear-selection').forEach((node) => { node.disabled = isRunning(); });
}
async function refreshPlan() {
  const request = ++state.planRequest;
  if (!state.selected.size || !state.settings) {
    state.plan = null; state.planError = ''; renderSelection(); return;
  }
  try {
    const plan = await invoke('plan_run', {tasks: Array.from(state.selected)});
    if (request !== state.planRequest) return;
    state.plan = plan; state.planError = '';
  } catch (error) {
    if (request !== state.planRequest) return;
    state.plan = null; state.planError = errorText(error);
  }
  renderSelection();
}
function applyPreset(name) {
  const presets = {
    media: ['images', 'audio', 'models'], data: ['languages', 'maps', 'items', 'calendars'],
    all: TOOLS.map((entry) => entry.id)
  };
  if (!presets[name]) return;
  state.selected = new Set(presets[name]);
  state.plan = null;
  document.querySelectorAll('[data-preset]').forEach((button) => button.classList.toggle('active', button.dataset.preset === name));
  saveSelection(); renderSelection(); refreshPlan();
}
async function navigate(page) {
  if (!['extract', 'cache', 'library', 'activity', 'settings'].includes(page)) return false;
  if (page === state.page) return true;
  if (!await allowSettingsExit()) return false;
  if (state.page === 'library' && page !== 'library') closePreview();
  if (state.page === 'cache' && page !== 'cache' && typeof closeCachePreview === 'function') closeCachePreview();
  state.page = page;
  document.querySelectorAll('.page').forEach((node) => { node.hidden = node.id !== 'page-' + page; });
  document.querySelectorAll('.nav-item').forEach((node) => {
    node.classList.toggle('active', node.dataset.page === page);
    node.setAttribute('aria-current', node.dataset.page === page ? 'page' : 'false');
  });
  $('page-title').textContent = {extract: 'Extract', cache: 'Game cache', library: 'File library', activity: 'Activity', settings: 'Settings'}[page];
  $('library-shortcuts').hidden = page !== 'library';
  if (page === 'library') loadDirectory();
  if (page === 'cache' && typeof loadCacheBrowser === 'function' && !cacheBrowser.catalog && !cacheBrowser.loading) loadCacheBrowser();
  if (page === 'settings') fillSettings();
  return true;
}
function applyBoot(boot) {
  const previousSettings = state.settings;
  state.boot = boot;
  state.settings = boot.settings;
  if (previousSettings && (previousSettings.workspace !== boot.settings.workspace || previousSettings.cache_path !== boot.settings.cache_path) && typeof resetCacheBrowser === 'function') resetCacheBrowser();
  $('workspace-name').textContent = basename(boot.settings.workspace);
  $('workspace-name').title = boot.settings.workspace;
  $('app-version').textContent = 'v' + boot.version;
  $('summary-output').textContent = prettyPath(boot.settings.output);
  $('summary-output').title = boot.settings.output;
  $('footer-output').textContent = prettyPath(boot.settings.output);
  $('footer-output').title = boot.settings.output;
  $('keep-intermediates').checked = boot.settings.keep_intermediates;
  const game = $('game-status');
  game.classList.toggle('not-found', !boot.game_found);
  game.querySelector('span:last-child').textContent = boot.game_found ? 'Soulframe detected' : 'Game cache not detected';
  game.title = boot.cache_path || 'Choose a game cache folder in Settings.';
  if (boot.runtime_libraries) renderRuntimeLibraries(boot.runtime_libraries);
  if (boot.run_environment) renderRunEnvironment(boot.run_environment);
  if (boot.settings_warning) toast(boot.settings_warning, 'error');
}
function settingsFingerprint(settings) {
  return JSON.stringify({
    workspace: String(settings.workspace || '').trim(), output: String(settings.output || '').trim(),
    cache_path: String(settings.cache_path || '').trim(), dll_directory: String(settings.dll_directory || '').trim(),
    locales: String(settings.locales || '').split(',').map((locale) => locale.trim()).filter(Boolean).sort(),
    keep_intermediates: Boolean(settings.keep_intermediates)
  });
}
function hasUnsavedSettings() {
  return state.page === 'settings' && state.settings && state.settingsBaseline !== null &&
    settingsFingerprint(draftSettings()) !== state.settingsBaseline;
}
function updateSettingsDirty() { $('settings-unsaved').hidden = !hasUnsavedSettings(); }
function updateSettingsControls() {
  const blocked = isRunning() || state.settingsSaving;
  $('save-settings').disabled = blocked;
  $('save-settings-label').textContent = state.settingsSaving ? 'Saving…' : 'Save changes';
  document.querySelectorAll('.settings-page input:not([name="theme"]),.settings-page button[data-browse]').forEach((node) => { node.disabled = blocked; });
  document.querySelectorAll('input[name="theme"]').forEach((node) => { node.checked = node.value === document.documentElement.dataset.theme; });
  $('reset-dll-directory').disabled = blocked;
}
let settingsChoiceResolver = null;
function finishSettingsChoice(choice) {
  if (!settingsChoiceResolver) return;
  const resolve = settingsChoiceResolver;
  settingsChoiceResolver = null;
  $('unsaved-settings-dialog').close();
  resolve(choice);
}
function chooseSettingsExit() {
  return new Promise((resolve) => {
    settingsChoiceResolver = resolve;
    $('unsaved-settings-dialog').showModal();
    $('unsaved-settings-keep').focus();
  });
}
async function allowSettingsExit() {
  if (state.settingsSaving || state.settingsExitPending) return false;
  if (!hasUnsavedSettings()) return true;
  state.settingsExitPending = true;
  try {
    const choice = await chooseSettingsExit();
    if (choice === 'save') return await saveSettings();
    if (choice === 'discard') { fillSettings(); return true; }
    return false;
  } finally { state.settingsExitPending = false; }
}
async function guardWindowClose(event, currentWindow) {
  if (!hasUnsavedSettings() && !state.settingsSaving && !state.settingsExitPending) return;
  event.preventDefault();
  if (await allowSettingsExit()) {
    try { await currentWindow.destroy(); }
    catch (error) { toast('Could not close Soulkit: ' + errorText(error), 'error'); }
  }
}
async function wireWindowClose() {
  const api = window.__TAURI__ && window.__TAURI__.window;
  if (!api) return;
  const currentWindow = api.getCurrentWindow();
  await currentWindow.onCloseRequested((event) => guardWindowClose(event, currentWindow));
}
function fillSettings() {
  if (!state.settings) return;
  for (const name of ['workspace', 'output']) $('setting-' + name).value = state.settings[name] || '';
  $('setting-cache').value = state.settings.cache_path || '';
  $('setting-dll-directory').value = state.settings.dll_directory || '';
  $('setting-keep').checked = state.settings.keep_intermediates;
  const enabled = new Set(state.settings.locales.split(',').map((locale) => locale.trim()));
  document.querySelectorAll('#locale-options input').forEach((input) => { input.checked = enabled.has(input.value); });
  state.settingsBaseline = settingsFingerprint(draftSettings());
  updateSettingsDirty(); updateSettingsControls();
  checkRuntimeLibraries();
  checkRunEnvironment();
}
function draftSettings() {
  return {
    workspace: $('setting-workspace').value.trim(),
    output: $('setting-output').value.trim(),
    soulkit: state.settings.soulkit,
    cache_path: $('setting-cache').value.trim() || null,
    dll_directory: $('setting-dll-directory').value.trim() || null,
    locales: Array.from(document.querySelectorAll('#locale-options input:checked')).map((node) => node.value).join(','),
    keep_intermediates: $('setting-keep').checked
  };
}
function renderRunEnvironment(environment) {
  state.environment = environment;
  for (const [prefix, storage] of [['output', environment.output_storage], ['scratch', environment.scratch_storage]]) {
    const location = $(prefix + '-storage-path'), free = $(prefix + '-storage-free');
    location.textContent = storage ? prettyPath(storage.path) : '';
    location.title = storage && (storage.checked_path || storage.path) || '';
    const available = storage && storage.available_bytes;
    free.textContent = available === null || available === undefined ? 'Unavailable' : bytes(available) + ' free';
    free.classList.toggle('no-space', available === 0);
    free.title = storage && storage.error || (storage && storage.total_bytes !== null && storage.total_bytes !== undefined ? bytes(storage.total_bytes) + ' total' : '');
  }
  const cache = environment.cache || {};
  $('cache-detection').classList.toggle('missing', !cache.found);
  $('cache-detection').textContent = cache.found ? 'Cache found: ' + prettyPath(cache.path) : cache.error || 'No game cache found. Choose the installed Cache.Windows folder.';
}
async function checkRunEnvironment() {
  if (!state.settings) return;
  const request = ++state.environmentRequest;
  $('refresh-storage').disabled = true;
  $('storage-feedback').hidden = true;
  try {
    const environment = await invoke('inspect_run_environment', {settings: draftSettings()});
    if (request !== state.environmentRequest) return;
    renderRunEnvironment(environment);
  } catch (error) {
    if (request !== state.environmentRequest) return;
    $('output-storage-free').textContent = 'Unavailable';
    $('scratch-storage-free').textContent = 'Unavailable';
    $('storage-feedback').textContent = errorText(error);
    $('storage-feedback').hidden = false;
    $('cache-detection').textContent = 'Cache location could not be checked.';
    $('cache-detection').classList.add('missing');
  } finally {
    if (request === state.environmentRequest) $('refresh-storage').disabled = false;
  }
}
function renderRuntimeLibraries(report) {
  const list = $('runtime-library-list');
  list.replaceChildren();
  const files = report.files || [];
  const found = files.filter((file) => file.found).length;
  $('dll-status-summary').textContent = found + ' of ' + files.length + ' files found';
  $('dll-status-summary').classList.toggle('missing', found < files.length);
  $('dll-location').textContent = prettyPath(report.directory);
  for (const file of files) {
    const row = element('div', 'runtime-library-row');
    const glyph = element('span', 'runtime-library-icon ' + (file.found ? 'found' : 'missing'));
    glyph.innerHTML = icon(file.found ? 'check' : 'alert');
    const copy = element('div', 'runtime-library-copy');
    copy.append(element('strong', '', file.name), element('span', 'runtime-library-path', prettyPath(file.path)));
    row.append(glyph, copy, element('span', 'runtime-library-status ' + (file.found ? 'found' : 'missing'), file.found ? 'Found' : 'Missing'));
    list.append(row);
  }
}
async function checkRuntimeLibraries() {
  const request = ++state.dllRequest;
  const directory = $('setting-dll-directory').value.trim() || null;
  const saved = state.settings ? state.settings.dll_directory || null : null;
  $('dll-unsaved').hidden = prettyPath(directory).toLowerCase() === prettyPath(saved).toLowerCase();
  $('dll-feedback').hidden = true;
  $('check-runtime-libraries').disabled = true;
  $('dll-status-summary').textContent = 'Checking files…';
  try {
    const report = await invoke('inspect_runtime_libraries', {directory});
    if (request !== state.dllRequest) return;
    renderRuntimeLibraries(report);
  } catch (error) {
    if (request !== state.dllRequest) return;
    $('runtime-library-list').replaceChildren();
    $('dll-location').textContent = '';
    $('dll-status-summary').textContent = 'Could not check files';
    $('dll-feedback').textContent = errorText(error);
    $('dll-feedback').hidden = false;
  } finally {
    if (request === state.dllRequest) $('check-runtime-libraries').disabled = false;
  }
}
async function saveSettings() {
  if (isRunning()) { toast('Wait for the current extraction to finish before changing folders.', 'error'); return false; }
  if (state.settingsSaving) return false;
  const locales = Array.from(document.querySelectorAll('#locale-options input:checked')).map((node) => node.value).join(',');
  if (!locales) { toast('Select at least one language pack.', 'error'); return false; }
  const settings = draftSettings();
  state.settingsSaving = true; updateSettingsControls();
  try {
    const oldOutput = state.settings.output;
    const boot = await invoke('save_settings', {settings});
    applyBoot(boot);
    fillSettings();
    if (oldOutput !== boot.settings.output) {
      state.directory = ''; state.search = ''; $('file-search').value = ''; closePreview();
    }
    refreshPlan(); refreshStats(); toast('Workspace settings saved.');
    return true;
  } catch (error) { toast(errorText(error), 'error'); return false; }
  finally { state.settingsSaving = false; updateSettingsControls(); updateSettingsDirty(); }
}
async function browseSetting(name) {
  const field = $(name === 'cache_path' ? 'setting-cache' : name === 'dll_directory' ? 'setting-dll-directory' : 'setting-' + name);
  try {
    const path = await invoke('select_folder', {initial: field.value || null});
    if (path) {
      field.value = path;
      updateSettingsDirty();
      if (name === 'dll_directory') checkRuntimeLibraries();
      if (['workspace', 'output', 'cache_path'].includes(name)) checkRunEnvironment();
    }
  } catch (error) { toast(errorText(error), 'error'); }
}

async function startRun() {
  if (isRunning() || !state.selected.size || !state.plan) return;
  state.runPlan = state.plan;
  state.logs = []; renderLog();
  state.run = {status: 'starting', progress: 0, job_index: 0, total_jobs: state.plan.jobs.length, label: 'Preparing extraction'};
  renderRun(); renderLog();
  try {
    state.runPlan = await invoke('start_run', {tasks: Array.from(state.selected)});
    renderRun();
  } catch (error) {
    const failure = runFailure(error);
    state.run = {status: 'failed', progress: 0, job_index: 0, label: failure.title, error: failure.message, failure};
    renderRun(); renderLog(); toast(failure.message, 'error');
  }
}
async function cancelRun() {
  try { await invoke('cancel_run'); }
  catch (error) { toast(errorText(error), 'error'); }
}
function acceptEvent(payload) {
  if (payload.kind === 'log' && payload.line !== null && payload.line !== undefined) {
    const line = String(payload.line).replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
    state.logs.push(line);
    if (state.logs.length > 3000) state.logs.splice(0, state.logs.length - 3000);
    $('latest-log').textContent = line;
    if (!state.logTimer) state.logTimer = setTimeout(() => { state.logTimer = null; renderLog(); }, 90);
  }
  if (payload.status) {
    const previous = state.run.status;
    state.run = payload.status;
    scheduleRunRender();
    if (previous !== state.run.status && ['completed', 'failed', 'cancelled'].includes(state.run.status)) {
      if (state.run.status === 'completed') toast('Extraction complete. Your files are ready in the library.');
      if (state.run.status === 'failed') toast(runFailure(state.run.failure || state.run.error).message, 'error');
      if (state.run.status === 'cancelled') toast('Extraction stopped. Files already written are preserved.');
      refreshStats(); refreshPlan();
      if (state.page === 'library') loadDirectory();
    }
  }
}
let runRenderPending = false;
function scheduleRunRender() {
  if (runRenderPending) return;
  runRenderPending = true;
  requestAnimationFrame(() => { runRenderPending = false; renderRun(); });
}
function renderRun() {
  const running = isRunning(), status = state.run.status || 'idle';
  const activePlan = state.runPlan || state.plan;
  const failure = status === 'failed' ? runFailure(state.run.failure || state.run.error) : null;
  const progress = Math.max(0, Math.min(1, Number(state.run.progress) || 0));
  const labels = {idle: 'Ready', starting: 'Starting', running: 'Extracting', cancelling: 'Stopping', completed: 'Complete', failed: 'Needs attention', cancelled: 'Stopped'};
  $('global-status').className = 'status-pill ' + status;
  $('global-status').replaceChildren(element('span', 'status-dot'), document.createTextNode(labels[status] || status));
  $('footer-status').textContent = running ? state.run.label || 'Extraction in progress' :
    status === 'completed' ? 'Extraction complete' : failure ? failure.title : 'Ready';
  $('activity-indicator').hidden = !running;
  $('extraction-progress').hidden = status === 'idle' || Boolean(failure);
  $('extraction-progress-label').textContent = state.run.label || labels[status] || 'Extraction';
  $('extraction-progress-value').textContent = Math.round(progress * 100) + '%';
  $('extraction-progress-bar').style.width = progress * 100 + '%';
  $('activity-progress-bar').style.width = progress * 100 + '%';
  $('activity-progress-value').textContent = status === 'idle' || failure ? '' : Math.round(progress * 100) + '%';
  $('activity-overview').hidden = status === 'idle';
  $('activity-progress-track').hidden = status === 'idle' || Boolean(failure);
  $('extract-cancel').hidden = !running;
  $('activity-cancel').hidden = !running;
  $('extract-cancel').disabled = status === 'cancelling';
  $('activity-cancel').disabled = status === 'cancelling';
  $('activity-status-label').textContent = status === 'idle' ? '' : labels[status] || status;
  $('activity-title').textContent = failure ? failure.title : status === 'idle' ? '' : state.run.label || labels[status];
  $('activity-description').textContent = failure ? failure.message : state.run.error || (status === 'idle' ? '' :
    status === 'completed' ? 'All selected jobs finished successfully.' :
    status === 'cancelled' ? 'The run was stopped. Completed files remain in your output folder.' :
    'Job ' + (state.run.job_index || 1) + ' of ' + (state.run.total_jobs || (activePlan && activePlan.jobs.length) || 1));
  $('keep-intermediates').disabled = running;
  const list = $('job-list');
  list.replaceChildren();
  if (activePlan && status !== 'idle' && !(failure && !state.run.job_index)) {
    activePlan.jobs.forEach((job, index) => {
      const current = Math.max(0, Number(state.run.job_index || 1) - 1);
      let jobStatus = index < current || status === 'completed' ? 'completed' : index === current ? status : 'pending';
      if (jobStatus === 'starting') jobStatus = 'running';
      const row = element('div', 'job-row ' + jobStatus);
      row.innerHTML = icon(jobStatus === 'completed' ? 'check' : jobStatus === 'failed' ? 'alert' : 'clock');
      row.append(element('span', '', job.label), element('span', 'job-state', {completed: 'Done', pending: 'Queued', running: 'Running', failed: 'Failed', cancelled: 'Stopped', cancelling: 'Stopping'}[jobStatus] || jobStatus));
      list.append(row);
    });
  }
  renderFailure(failure);
  $('copy-log').disabled = status === 'idle' && !state.logs.length;
  renderSelection();
  if (typeof renderCacheRun === 'function') renderCacheRun();
  if (state.page === 'settings') updateSettingsControls();
}
function renderLog() {
  $('log-count').textContent = state.logs.length.toLocaleString() + ' lines';
  $('run-log').textContent = state.logs.length ? state.logs.join('\n') : state.run.status === 'idle' ? 'No runs yet.' : state.run.status === 'failed' ?
    state.run.job_index ? 'No worker output was captured.' : 'The run did not start. Resolve the issue above and try again.' : 'Waiting for output…';
  if ($('follow-log').checked) $('run-log').scrollTop = $('run-log').scrollHeight;
}

async function refreshStats() {
  try {
    state.stats = await invoke('library_stats');
    $('nav-file-count').textContent = state.stats.files ? Number(state.stats.files).toLocaleString() : '';
    $('library-subtitle').textContent = state.stats.files ?
      Number(state.stats.files).toLocaleString() + (state.stats.truncated ? '+ files' : ' files') + ' · ' + bytes(state.stats.bytes) :
      'Extracted files';
  } catch (_) {}
}
async function loadDirectory() {
  const request = ++state.libraryRequest;
  $('file-loading').hidden = false;
  $('file-grid').setAttribute('aria-busy', 'true');
  renderBreadcrumbs();
  try {
    const entries = state.search.trim() ?
      await invoke('search_files', {query: state.search.trim(), limit: 1000}) :
      await invoke('list_directory', {relative: state.directory});
    if (request !== state.libraryRequest) return;
    state.entries = entries;
    $('search-limit').hidden = !state.search || entries.length < 1000;
    renderFiles();
  } catch (error) {
    if (request !== state.libraryRequest) return;
    state.entries = []; renderFiles();
    $('library-empty').querySelector('p').textContent = errorText(error);
  } finally {
    if (request === state.libraryRequest) {
      $('file-loading').hidden = true;
      $('file-grid').setAttribute('aria-busy', 'false');
    }
  }
}
function renderBreadcrumbs() {
  const container = $('breadcrumbs');
  container.replaceChildren();
  const root = element('button', '', 'Output');
  root.addEventListener('click', () => goFolder(''));
  container.append(root);
  if (state.search.trim()) {
    const chevron = element('span', 'breadcrumb-separator'); chevron.innerHTML = icon('chevron-right');
    container.append(chevron, element('span', '', 'Search results'));
  } else {
    let relative = '';
    for (const part of state.directory.split('/').filter(Boolean)) {
      relative += (relative ? '/' : '') + part;
      const target = relative;
      const button = element('button', '', part);
      button.addEventListener('click', () => goFolder(target));
      const chevron = element('span', 'breadcrumb-separator'); chevron.innerHTML = icon('chevron-right');
      container.append(chevron, button);
    }
  }
  $('folder-up').disabled = !state.directory && !state.search;
}
function goFolder(relative) {
  state.directory = relative.replace(/\\/g, '/');
  state.search = ''; $('file-search').value = '';
  closePreview(); loadDirectory();
}
function sortedEntries() {
  const selected = state.entries.slice();
  const sort = $('file-sort').value;
  selected.sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    if (sort === 'size') return b.size - a.size || a.name.localeCompare(b.name);
    if (sort === 'newest') return (b.modified || 0) - (a.modified || 0) || a.name.localeCompare(b.name);
    return a.name.localeCompare(b.name, undefined, {numeric: true, sensitivity: 'base'});
  });
  return selected;
}
function renderFiles() {
  const entries = sortedEntries();
  const grid = $('file-grid');
  grid.className = 'file-grid' + (state.view === 'list' ? ' list-view' : '');
  grid.hidden = entries.length === 0;
  grid.replaceChildren();
  $('library-empty').hidden = entries.length > 0;
  $('library-empty').querySelector('h2').textContent = state.search ? 'No matching files' : 'No files here yet';
  $('library-empty').querySelector('p').textContent = state.search ? 'Try a different search.' : 'Run an extraction and your files will appear here.';
  $('visible-file-count').textContent = entries.length.toLocaleString() + (entries.length === 1 ? ' item' : ' items');
  // Chunk large directories so the interface remains interactive while tiles render.
  const request = state.libraryRequest;
  const renderRequest = ++state.fileRenderRequest;
  let offset = 0;
  function appendChunk() {
    if (request !== state.libraryRequest || renderRequest !== state.fileRenderRequest) return;
    for (const entry of entries.slice(offset, offset + 100)) grid.append(fileCard(entry));
    offset += 100;
    if (offset < entries.length) requestAnimationFrame(appendChunk);
  }
  appendChunk();
}
function fileCard(entry) {
  const card = element('button', 'file-card ' + (entry.is_dir ? 'folder' : entry.kind));
  card.classList.toggle('selected', Boolean(state.selectedFile && state.selectedFile.relative_path === entry.relative_path));
  card.title = entry.relative_path;
  card.setAttribute('aria-label', (entry.is_dir ? 'Open folder ' : 'Preview ') + entry.name);
  const thumb = element('div', 'file-thumb');
  const kindIcon = {folder: 'folder', image: 'image', audio: 'music', json: 'code', text: 'file', binary: 'file'}[entry.kind] || 'file';
  if (entry.kind === 'image' && !entry.is_dir) {
    const image = element('img');
    image.src = assetUrl(entry.absolute_path);
    image.loading = 'lazy'; image.decoding = 'async'; image.alt = '';
    image.addEventListener('error', () => { image.remove(); thumb.innerHTML = icon('image'); }, {once: true});
    thumb.append(image);
  } else {
    thumb.innerHTML = icon(kindIcon);
    if (entry.kind === 'audio') {
      const bars = element('span', 'audio-bars');
      for (let i = 0; i < 11; i += 1) bars.append(element('b'));
      thumb.append(bars);
    }
  }
  if (!entry.is_dir) thumb.append(element('span', 'file-ext', entry.name.split('.').pop().toUpperCase().slice(0, 8)));
  const details = element('div', 'file-card-details');
  details.append(element('span', 'file-name', entry.name));
  const meta = state.search ? entry.relative_path : entry.is_dir ? 'Folder' : bytes(entry.size);
  details.append(element('span', 'file-meta', meta));
  card.append(thumb, details);
  card.addEventListener('click', () => entry.is_dir ? goFolder(entry.relative_path) : selectFile(entry, card));
  card.addEventListener('dblclick', () => { if (!entry.is_dir) openSelected(entry); });
  return card;
}
async function selectFile(entry, card) {
  state.selectedFile = entry;
  document.querySelectorAll('.file-card.selected').forEach((node) => node.classList.remove('selected'));
  if (card) card.classList.add('selected');
  $('inspector-empty').hidden = true;
  $('inspector-content').hidden = false;
  $('preview-name').textContent = entry.name;
  $('preview-relative').textContent = entry.relative_path;
  $('preview-size').textContent = bytes(entry.size);
  $('preview-modified').textContent = entry.modified ? new Date(entry.modified).toLocaleDateString(undefined, {year: 'numeric', month: 'short', day: 'numeric'}) : '—';
  $('preview-kind').textContent = {image: 'IMAGE', audio: 'AUDIO', json: 'JSON DATA', text: 'TEXT', binary: 'BINARY'}[entry.kind] || entry.kind.toUpperCase();
  $('preview-notice').hidden = true;
  const stage = $('preview-stage');
  stopAudio(); stage.replaceChildren();
  const request = ++state.previewRequest;
  if (entry.kind === 'image') {
    stage.className = 'preview-stage image-preview';
    const image = element('img'); image.src = assetUrl(entry.absolute_path); image.alt = entry.name;
    image.addEventListener('error', () => {
      image.remove(); stage.append(element('p', '', 'This image format cannot be previewed here. Open the original file to view it.'));
    }, {once: true});
    stage.append(image); return;
  }
  if (entry.kind === 'audio') {
    stage.className = 'preview-stage audio-preview';
    const art = element('span', 'audio-art'); art.innerHTML = icon('headphones');
    const audio = element('audio'); audio.controls = true; audio.preload = 'metadata'; audio.src = assetUrl(entry.absolute_path);
    const note = element('p', '', 'Listen without leaving your collection.');
    audio.addEventListener('error', () => {
      note.textContent = 'This codec is not supported by your installed WebView. Use Open file to listen in your audio player.';
    });
    stage.append(art, audio, note); return;
  }
  stage.className = 'preview-stage text-preview';
  stage.append(element('p', 'preview-loading', 'Loading preview…'));
  try {
    const preview = await invoke('preview_file', {relative: entry.relative_path});
    if (request !== state.previewRequest) return;
    stage.replaceChildren(element('pre', entry.kind === 'binary' ? 'binary-preview' : '', preview.content));
    if (preview.truncated) {
      $('preview-notice').hidden = false;
      $('preview-notice').textContent = 'This is a limited preview. Open the original file to view its full contents.';
    }
  } catch (error) {
    if (request === state.previewRequest) stage.replaceChildren(element('p', '', errorText(error)));
  }
}
function stopAudio() {
  document.querySelectorAll('#preview-stage audio').forEach((audio) => { audio.pause(); audio.removeAttribute('src'); audio.load(); });
}
function closePreview() {
  state.previewRequest += 1; stopAudio(); state.selectedFile = null;
  $('preview-stage').replaceChildren();
  $('inspector-empty').hidden = false; $('inspector-content').hidden = true;
  document.querySelectorAll('.file-card.selected').forEach((node) => node.classList.remove('selected'));
}
async function openSelected(entry = state.selectedFile) {
  if (!entry) return;
  try { await invoke('open_file', {relative: entry.relative_path}); }
  catch (error) { toast(errorText(error), 'error'); }
}
async function openOutput() {
  try { await invoke('open_output'); } catch (error) { toast(errorText(error), 'error'); }
}

function wireEvents() {
  paintIcons();
  if (typeof wireCacheEvents === 'function') wireCacheEvents();
  document.addEventListener('click', async (event) => {
    const nav = event.target.closest('[data-page]');
    if (nav) { event.preventDefault(); navigate(nav.dataset.page); }
    const folder = event.target.closest('[data-folder]');
    if (folder && await navigate('library')) goFolder(folder.dataset.folder);
  });
  document.querySelector('.brand').addEventListener('click', (event) => { event.preventDefault(); navigate('extract'); });
  document.querySelectorAll('[data-preset]').forEach((node) => node.addEventListener('click', () => applyPreset(node.dataset.preset)));
  $('clear-selection').addEventListener('click', () => { state.selected.clear(); state.plan = null; saveSelection(); renderSelection(); refreshPlan(); });
  $('workspace-settings').addEventListener('click', () => navigate('settings'));
  $('summary-output').addEventListener('click', async () => { if (await navigate('settings')) $('setting-output').focus(); });
  $('top-open-output').addEventListener('click', openOutput);
  $('library-open-output').addEventListener('click', openOutput);
  $('run-button').addEventListener('click', startRun);
  $('extract-cancel').addEventListener('click', cancelRun);
  $('activity-cancel').addEventListener('click', cancelRun);
  $('save-settings').addEventListener('click', saveSettings);
  $('unsaved-settings-keep').addEventListener('click', () => finishSettingsChoice('keep'));
  $('unsaved-settings-discard').addEventListener('click', () => finishSettingsChoice('discard'));
  $('unsaved-settings-save').addEventListener('click', () => finishSettingsChoice('save'));
  $('unsaved-settings-dialog').addEventListener('cancel', (event) => { event.preventDefault(); finishSettingsChoice('keep'); });
  document.querySelectorAll('.settings-page input:not([name="theme"])').forEach((node) => {
    node.addEventListener('input', updateSettingsDirty);
    node.addEventListener('change', updateSettingsDirty);
  });
  document.querySelectorAll('[data-browse]').forEach((node) => node.addEventListener('click', () => browseSetting(node.dataset.browse)));
  document.querySelectorAll('input[name="theme"]').forEach((node) => node.addEventListener('change', () => {
    if (node.checked) window.SoulkitTheme.set(node.value);
  }));
  $('keep-intermediates').addEventListener('change', async () => {
    try { applyBoot(await invoke('save_settings', {settings: Object.assign({}, state.settings, {keep_intermediates: $('keep-intermediates').checked})})); refreshPlan(); }
    catch (error) { $('keep-intermediates').checked = state.settings.keep_intermediates; toast(errorText(error), 'error'); }
  });
  $('refresh-library').addEventListener('click', () => { loadDirectory(); refreshStats(); });
  let searchTimer;
  $('file-search').addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { state.search = $('file-search').value; loadDirectory(); }, 250);
  });
  $('check-runtime-libraries').addEventListener('click', checkRuntimeLibraries);
  $('refresh-storage').addEventListener('click', checkRunEnvironment);
  let environmentTimer;
  for (const id of ['setting-workspace', 'setting-output', 'setting-cache']) {
    $(id).addEventListener('input', () => {
      clearTimeout(environmentTimer); environmentTimer = setTimeout(checkRunEnvironment, 350);
    });
  }
  let dllTimer;
  $('setting-dll-directory').addEventListener('input', () => {
    clearTimeout(dllTimer); dllTimer = setTimeout(checkRuntimeLibraries, 300);
  });
  $('reset-dll-directory').addEventListener('click', () => {
    $('setting-dll-directory').value = ''; updateSettingsDirty(); checkRuntimeLibraries();
  });
  $('file-sort').addEventListener('change', renderFiles);
  $('grid-view').addEventListener('click', () => setView('grid'));
  $('list-view').addEventListener('click', () => setView('list'));
  $('folder-up').addEventListener('click', () => {
    if (state.search) goFolder(state.directory); else goFolder(state.directory.split('/').slice(0, -1).join('/'));
  });
  $('close-preview').addEventListener('click', closePreview);
  $('open-selected').addEventListener('click', () => openSelected());
  $('reveal-selected').addEventListener('click', async () => {
    if (!state.selectedFile) return;
    try { await invoke('reveal_file', {relative: state.selectedFile.relative_path}); }
    catch (error) { toast(errorText(error), 'error'); }
  });
  $('copy-log').addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(runReport()); toast('Run report copied.'); }
    catch (error) { toast('Could not copy the report: ' + errorText(error), 'error'); }
  });
  document.addEventListener('keydown', async (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); if (state.page === 'cache') $('cache-search').focus(); else if (await navigate('library')) $('file-search').focus(); }
    if (event.key === 'Escape' && state.page === 'library') closePreview();
    if (event.key === 'Escape' && state.page === 'cache' && typeof closeCachePreview === 'function') closeCachePreview();
  });
  const localeRoot = $('locale-options');
  LOCALES.forEach(([id, label]) => {
    const node = element('label', 'locale-option');
    const input = element('input'); input.type = 'checkbox'; input.value = id;
    input.addEventListener('change', updateSettingsDirty);
    node.append(input, element('span', '', id.toUpperCase()));
    node.title = label; localeRoot.append(node);
  });
}
function setView(view) {
  state.view = view;
  $('grid-view').classList.toggle('active', view === 'grid');
  $('list-view').classList.toggle('active', view === 'list');
  renderFiles();
}
async function initialize() {
  wireEvents();
  try {
    const selection = JSON.parse(localStorage.getItem('soulkit.selected'));
    if (Array.isArray(selection)) state.selected = new Set(selection.filter((id) => tool(id)));
  } catch (_) {}
  renderTools(); renderSelection();
  try {
    if (!window.__TAURI__) throw new Error('This is the desktop interface. Build and open soulkit.exe to connect to your local files.');
    await wireWindowClose();
    await window.__TAURI__.event.listen('run-event', (event) => acceptEvent(event.payload));
    const boot = await invoke('get_settings');
    applyBoot(boot);
    state.run = await invoke('run_status');
    renderRun();
    await refreshPlan();
    refreshStats();
  } catch (error) {
    $('workspace-name').textContent = 'Workspace unavailable';
    state.planError = errorText(error); renderSelection();
    toast(errorText(error), 'error');
  } finally { $('initial-loading').hidden = true; }
}
initialize();
