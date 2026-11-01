'use strict';

// The catalog and selection stay in the native process. Only one page of metadata
// crosses IPC; previews decode a single asset after an explicit click.
const cacheBrowser = {
  generation: null, folder: '', query: '', kind: 'all', offset: 0, limit: 100,
  catalog: null, selection: {count: 0, images: 0, audio: 0, models: 0, compressed_bytes: 0},
  loading: false, mutating: false, starting: false, request: 0, previewRequest: 0,
  preview: null, error: '', valid: false, searchTimer: null, cancelPending: Promise.resolve()
};

function cacheScope() {
  return {folder: cacheBrowser.folder, query: cacheBrowser.query.trim(), kind: cacheBrowser.kind};
}
function cacheBlocked() { return cacheBrowser.loading || cacheBrowser.mutating || cacheBrowser.starting || isRunning(); }
function cacheCount(count) { return Number(count || 0).toLocaleString(); }
function cacheError(error) {
  cacheBrowser.error = errorText(error);
  $('cache-error-message').textContent = cacheBrowser.error;
  $('cache-error').hidden = !cacheBrowser.error;
}
function renderCacheControls() {
  const blocked = cacheBlocked(), data = cacheBrowser.catalog;
  const count = Number(cacheBrowser.selection.count || 0);
  $('cache-refresh').disabled = cacheBrowser.mutating || cacheBrowser.starting || isRunning();
  $('cache-scan').disabled = blocked;
  $('cache-up').disabled = blocked || !cacheBrowser.valid || !cacheBrowser.folder;
  $('cache-select-scope').disabled = blocked || !data || !cacheBrowser.valid;
  $('cache-select-all').disabled = blocked || !cacheBrowser.valid;
  $('cache-clear-selection').disabled = blocked || !count || !cacheBrowser.valid;
  $('cache-extract').disabled = blocked || !cacheBrowser.valid || !count;
  $('cache-image-format').disabled = blocked || !cacheBrowser.selection.images;
  $('cache-kind').disabled = cacheBrowser.loading || cacheBrowser.mutating || cacheBrowser.starting;
  $('cache-search').disabled = cacheBrowser.loading || cacheBrowser.mutating || cacheBrowser.starting;
  $('cache-previous').disabled = blocked || !cacheBrowser.valid || !data || !data.offset;
  $('cache-next').disabled = blocked || !cacheBrowser.valid || !data || data.offset + data.entries.length >= data.total;
  $('cache-preview-select').disabled = blocked || !cacheBrowser.preview || !cacheBrowser.valid;
  document.querySelectorAll('#cache-rows input,#cache-rows button,#cache-breadcrumbs button').forEach(node => { node.disabled = blocked || !cacheBrowser.valid; });
  $('cache-loading').hidden = !cacheBrowser.loading;
  $('cache-rows').setAttribute('aria-busy', String(cacheBrowser.loading));
  $('cache-select-scope').textContent = cacheBrowser.query.trim() ? 'Select all search results' : cacheBrowser.folder ? 'Select folder contents' : 'Select all matching';
  $('cache-select-all').textContent = 'Select whole game';
  $('cache-select-scope').title = data ? 'Select every matching asset in this folder and its subfolders, including other pages (' + cacheCount(data.matching_assets) + ' assets).' : '';
  $('cache-select-all').title = data ? 'Add all ' + cacheCount(data.total_assets) + ' images, audio and model assets in the game to your selection.' : '';
  $('cache-selection-count').textContent = count ? cacheCount(count) + (count === 1 ? ' asset selected' : ' assets selected') : 'Nothing selected';
  const selection = cacheBrowser.selection;
  $('cache-selection-detail').textContent = count ? cacheCount(selection.images) + ' images · ' + cacheCount(selection.audio) + ' audio · ' + cacheCount(selection.models) + ' models · ' + bytes(selection.compressed_bytes) + ' stored in cache' : 'Images → output/images · Audio → output/audio · Models → output/models';
  $('cache-selection-detail').title = 'Stored bytes describe the compressed cache data, not the space required for exported files.';
  $('cache-preview-select').textContent = 'Add to selection';
}
function renderCacheBreadcrumbs() {
  const root = $('cache-breadcrumbs'); root.replaceChildren();
  const button = element('button', '', 'Game cache');
  button.addEventListener('click', () => openCacheFolder('')); root.append(button);
  let path = '';
  for (const part of cacheBrowser.folder.split('/').filter(Boolean)) {
    path = path ? path + '/' + part : part;
    const target = path, separator = element('span', 'breadcrumb-separator'); separator.innerHTML = icon('chevron-right');
    const crumb = element('button', '', part === 'images' ? 'Images' : part === 'audio' ? 'Audio' : part === 'models' ? 'Models' : part);
    crumb.title = target; crumb.addEventListener('click', () => openCacheFolder(target)); root.append(separator, crumb);
  }
}
function renderCacheRows() {
  const container = $('cache-rows'), data = cacheBrowser.catalog;
  container.replaceChildren(); renderCacheBreadcrumbs();
  container.hidden = cacheBrowser.loading || !data || !data.entries.length;
  $('cache-empty').hidden = cacheBrowser.loading || Boolean(data && data.entries.length);
  $('cache-empty-text').textContent = cacheBrowser.error ? 'Try rescanning the cache or choose its folder in Settings.' : data ? (cacheBrowser.query.trim() ? 'No matching media in this folder.' : 'No media in this folder.') : 'Open the cache index to browse images, audio and models.';
  $('cache-scan').hidden = Boolean(data) || Boolean(cacheBrowser.error);
  $('cache-result-count').textContent = data ? cacheCount(data.total) + (cacheBrowser.query.trim() ? ' results' : ' entries') + ' · ' + cacheCount(data.matching_assets) + ' assets' : '';
  $('cache-page-count').textContent = data && data.total ? cacheCount(data.offset + 1) + '–' + cacheCount(data.offset + data.entries.length) + ' of ' + cacheCount(data.total) : '';
  for (const entry of data && data.entries || []) {
    const row = element('div', 'cache-row' + (entry.is_dir ? ' folder' : '') + (cacheBrowser.preview && cacheBrowser.preview.id === entry.id ? ' previewing' : ''));
    const check = element('input'); check.type = 'checkbox'; check.checked = Boolean(entry.selected); check.indeterminate = Boolean(entry.partial);
    check.setAttribute('aria-label', (entry.is_dir ? 'Select contents of ' : 'Select ') + entry.path);
    check.addEventListener('change', () => selectCacheAssets(check.checked ? 'add' : 'remove', {ids: [entry.id]}));
    const button = element('button', 'cache-row-open');
    const glyph = element('span', 'cache-row-icon'); glyph.innerHTML = icon(entry.is_dir ? 'folder' : entry.kind === 'images' ? 'image' : entry.kind === 'models' ? 'cube' : 'headphones');
    const copy = element('span', 'cache-row-copy');
    copy.append(element('strong', '', entry.name));
    const detail = cacheBrowser.query.trim() ? entry.path : entry.is_dir ? cacheCount(entry.count) + ' assets' : entry.category || (entry.kind === 'images' ? 'Image' : entry.kind === 'models' ? 'Model' : 'Audio');
    copy.append(element('span', '', detail));
    button.append(glyph, copy); button.title = entry.is_dir ? 'Open ' + entry.path : 'Preview ' + entry.path;
    button.addEventListener('click', () => entry.is_dir ? openCacheFolder(entry.path) : previewCacheAsset(entry));
    row.append(check, button, element('span', 'cache-row-size', bytes(entry.compressed_bytes)));
    container.append(row);
  }
  renderCacheControls();
}
async function loadCacheBrowser(refresh = false) {
  clearTimeout(cacheBrowser.searchTimer); cacheBrowser.searchTimer = null;
  const request = ++cacheBrowser.request;
  const generation = refresh ? null : cacheBrowser.generation;
  cacheBrowser.loading = true; cacheBrowser.catalog = null; cacheError('');
  if (refresh) { cacheBrowser.valid = false; closeCachePreview(); }
  $('cache-loading-label').textContent = refresh || !generation ? 'Reading cache indexes…' : 'Loading cache folder…';
  renderCacheRows();
  try {
    await cacheBrowser.cancelPending;
    if (request !== cacheBrowser.request) return;
    const data = await invoke('cache_browse', {request: {...cacheScope(), generation, offset: cacheBrowser.offset, limit: cacheBrowser.limit, refresh}});
    if (request !== cacheBrowser.request) return;
    if (!data || !Array.isArray(data.entries)) throw new Error('The cache returned an invalid catalog. Rescan the cache to try again.');
    // A changed catalog never inherits an older catalog's preview or selection.
    if (cacheBrowser.generation !== data.generation) closeCachePreview();
    cacheBrowser.generation = data.generation; cacheBrowser.catalog = data; cacheBrowser.selection = data.selection;
    cacheBrowser.offset = data.offset; cacheBrowser.valid = true;
    $('cache-source-path').textContent = prettyPath(data.cache_path); $('cache-source-path').title = prettyPath(data.cache_path);
  } catch (error) {
    if (request !== cacheBrowser.request) return;
    cacheBrowser.valid = false; cacheError(error);
  } finally {
    if (request === cacheBrowser.request) { cacheBrowser.loading = false; renderCacheRows(); }
  }
}
function cancelCacheLoading() {
  clearTimeout(cacheBrowser.searchTimer); cacheBrowser.searchTimer = null;
  cacheBrowser.request += 1; cacheBrowser.loading = false;
  cacheBrowser.cancelPending = invoke('cache_cancel_browse').catch(() => {});
  renderCacheRows();
}
function openCacheFolder(folder) {
  if (cacheBrowser.mutating || cacheBrowser.starting || isRunning()) return;
  cacheBrowser.folder = folder; cacheBrowser.query = ''; cacheBrowser.offset = 0;
  $('cache-search').value = ''; closeCachePreview(); loadCacheBrowser();
}
async function selectCacheAssets(action, scope = cacheScope()) {
  if (cacheBlocked() || !cacheBrowser.valid || cacheBrowser.generation === null) return;
  const generation = cacheBrowser.generation;
  cacheBrowser.mutating = true; cacheError(''); renderCacheControls();
  try {
    const selection = await invoke('cache_select', {request: {generation, action, ...scope}});
    if (generation !== cacheBrowser.generation) return;
    cacheBrowser.selection = selection;
  } catch (error) {
    if (generation !== cacheBrowser.generation) return;
    cacheError(error);
    // Refresh is required after any rejected mutation; never extract a stale selection.
    cacheBrowser.valid = false;
  } finally {
    cacheBrowser.mutating = false; renderCacheControls();
  }
  if (cacheBrowser.valid) await loadCacheBrowser();
}
function resetCacheBrowser() {
  clearTimeout(cacheBrowser.searchTimer); cacheBrowser.searchTimer = null;
  cacheBrowser.request += 1; cacheBrowser.generation = null; cacheBrowser.catalog = null;
  cacheBrowser.folder = ''; cacheBrowser.query = ''; cacheBrowser.offset = 0; cacheBrowser.valid = false;
  cacheBrowser.loading = false; cacheBrowser.selection = {count: 0, images: 0, audio: 0, models: 0, compressed_bytes: 0};
  $('cache-search').value = ''; cacheError(''); closeCachePreview(); renderCacheRows();
}
function closeCachePreview() {
  cacheBrowser.previewRequest += 1; cacheBrowser.preview = null;
  document.querySelectorAll('#cache-preview-stage audio').forEach(audio => { audio.pause(); audio.removeAttribute('src'); audio.load(); });
  $('cache-preview-stage').replaceChildren();
  $('cache-preview-empty').hidden = false; $('cache-preview-content').hidden = true;
  document.querySelectorAll('.cache-row.previewing').forEach(node => node.classList.remove('previewing'));
  // Cancellation also prevents a late preview from being added to private storage.
  return window.__TAURI__ ? invoke('cache_cancel_preview').catch(() => {}) : Promise.resolve();
}
async function previewCacheAsset(entry) {
  if (!cacheBrowser.valid || entry.is_dir) return;
  const cancellation = closeCachePreview(); cacheBrowser.preview = entry;
  const request = ++cacheBrowser.previewRequest, generation = cacheBrowser.generation;
  $('cache-preview-empty').hidden = true; $('cache-preview-content').hidden = false;
  $('cache-preview-name').textContent = entry.name; $('cache-preview-path').textContent = entry.path;
  $('cache-preview-size').textContent = bytes(entry.compressed_bytes);
  $('cache-preview-kind').textContent = {images: 'IMAGE', models: 'MODEL'}[entry.kind] || 'AUDIO';
  $('cache-preview-notice').textContent = 'Previews use temporary app storage. Extract to keep this asset in your output folder.';
  const stage = $('cache-preview-stage'); stage.className = 'preview-stage cache-preview-stage';
  stage.replaceChildren(element('span', 'spinner'), element('p', 'preview-loading', 'Decoding one asset…'));
  renderCacheRows();
  try {
    await cancellation;
    if (request !== cacheBrowser.previewRequest || generation !== cacheBrowser.generation) return;
    const preview = await invoke('cache_preview', {generation, id: entry.id});
    if (request !== cacheBrowser.previewRequest || generation !== cacheBrowser.generation) return;
    stage.replaceChildren();
    if (preview.notice) $('cache-preview-notice').textContent = preview.notice;
    if (!preview.path) { stage.append(element('p', '', preview.notice || 'This asset cannot be previewed. You can still extract it.')); return; }
    if (preview.kind === 'image') {
      stage.className = 'preview-stage cache-preview-stage image-preview';
      const image = element('img'); image.alt = entry.name; image.src = assetUrl(preview.path);
      image.addEventListener('error', () => { if (request === cacheBrowser.previewRequest) stage.replaceChildren(element('p', '', 'This image could not be displayed. You can still extract it.')); }, {once: true});
      stage.append(image);
    } else {
      stage.className = 'preview-stage cache-preview-stage audio-preview';
      const art = element('span', 'audio-art'); art.innerHTML = icon('headphones');
      const audio = element('audio'); audio.controls = true; audio.preload = 'metadata'; audio.src = assetUrl(preview.path);
      audio.addEventListener('error', () => { if (request === cacheBrowser.previewRequest) $('cache-preview-notice').textContent = 'This codec is not supported by your installed WebView. Extract it to open in an audio player.'; });
      stage.append(art, audio);
    }
  } catch (error) {
    if (request === cacheBrowser.previewRequest) stage.replaceChildren(element('p', '', errorText(error)));
  }
}
async function extractCacheSelection() {
  if (cacheBlocked() || !cacheBrowser.valid || !cacheBrowser.selection.count) return;
  const generation = cacheBrowser.generation;
  const imageFormat = $('cache-image-format').value;
  cacheBrowser.starting = true; cacheError('');
  state.runPlan = {requested: ['images', 'audio', 'models'].filter(kind => cacheBrowser.selection[kind]), dependencies: [], jobs: [{id: 'cache', label: 'Extract selected cache assets'}]};
  state.logs = []; state.run = {status: 'starting', progress: 0, job_index: 0, total_jobs: 1, label: 'Preparing selected cache assets'};
  renderRun(); renderLog(); renderCacheControls();
  try {
    state.runPlan = await invoke('cache_extract', {generation, imageFormat});
    renderRun();
  } catch (error) {
    const failure = runFailure(error);
    state.run = {status: 'failed', progress: 0, job_index: 0, label: failure.title, error: failure.message, failure};
    renderRun(); renderLog();
  } finally { cacheBrowser.starting = false; renderCacheControls(); }
}
function renderCacheRun() {
  renderCacheControls();
  const running = isRunning(), status = state.run.status;
  const failure = status === 'failed' ? runFailure(state.run.failure || state.run.error) : null;
  $('cache-run-status').hidden = status === 'idle';
  $('cache-run-status').classList.toggle('failed', Boolean(failure));
  $('cache-run-title').textContent = failure ? failure.title : state.run.label || {completed: 'Extraction complete', cancelled: 'Extraction stopped'}[status] || 'Extraction';
  $('cache-run-description').textContent = failure ? [failure.message, failure.suggestion].filter(Boolean).join(' ') : status === 'completed' ? 'Your extracted files are in the file library.' : status === 'cancelled' ? 'Files already written are preserved in your output folder.' : 'See Activity for progress and run output.';
  $('cache-run-cancel').hidden = !running; $('cache-run-cancel').disabled = status === 'cancelling';
}
function wireCacheEvents() {
  $('cache-scan').addEventListener('click', () => loadCacheBrowser());
  $('cache-refresh').addEventListener('click', () => { cacheBrowser.offset = 0; loadCacheBrowser(true); });
  $('cache-cancel-loading').addEventListener('click', cancelCacheLoading);
  $('cache-up').addEventListener('click', () => openCacheFolder(cacheBrowser.folder.split('/').slice(0, -1).join('/')));
  $('cache-settings').addEventListener('click', () => showFailureSettings('open_cache_settings'));
  $('cache-error-settings').addEventListener('click', () => showFailureSettings('open_cache_settings'));
  $('cache-select-scope').addEventListener('click', () => selectCacheAssets('add'));
  $('cache-select-all').addEventListener('click', () => selectCacheAssets('add', {folder: '', query: '', kind: 'all'}));
  $('cache-clear-selection').addEventListener('click', () => selectCacheAssets('clear', {}));
  $('cache-extract').addEventListener('click', extractCacheSelection);
  $('cache-run-cancel').addEventListener('click', cancelRun);
  $('cache-close-preview').addEventListener('click', closeCachePreview);
  $('cache-preview-select').addEventListener('click', () => {
    const entry = cacheBrowser.preview; if (!entry) return;
    selectCacheAssets('add', {ids: [entry.id]});
  });
  $('cache-previous').addEventListener('click', () => { cacheBrowser.offset = Math.max(0, cacheBrowser.offset - cacheBrowser.limit); loadCacheBrowser(); });
  $('cache-next').addEventListener('click', () => { cacheBrowser.offset += cacheBrowser.limit; loadCacheBrowser(); });
  $('cache-kind').addEventListener('change', () => { cacheBrowser.kind = $('cache-kind').value; cacheBrowser.offset = 0; loadCacheBrowser(); });
  $('cache-search').addEventListener('input', () => {
    clearTimeout(cacheBrowser.searchTimer);
    // Invalidate pending results immediately, including during the debounce.
    cacheBrowser.request += 1; cacheBrowser.catalog = null; cacheBrowser.loading = false;
    cacheBrowser.query = $('cache-search').value; cacheBrowser.offset = 0; renderCacheRows();
    cacheBrowser.searchTimer = setTimeout(() => loadCacheBrowser(), 250);
  });
  renderCacheRows();
}
