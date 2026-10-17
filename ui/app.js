// indir's window: the link field, and the list the app keeps sending.
'use strict';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const mac = navigator.userAgent.includes('Mac');
const rows = new Map();
let settings = { folder: '', connections: 8 };

/* Numbers in words */

function size(bytes) {
  if (bytes < 1000) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = -1;
  do {
    value /= 1000;
    unit += 1;
  } while (value >= 999.95 && unit < units.length - 1);
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

function duration(seconds) {
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

function finishedAt(unix) {
  const date = new Date(unix * 1000);
  if (date.toDateString() === new Date().toDateString()) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }
  return date.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

function badge(name) {
  const dot = name.lastIndexOf('.');
  const extension = dot > 0 ? name.slice(dot + 1) : '';
  if (!extension) return 'FILE';
  return (extension.length <= 4 ? extension : extension.slice(0, 3)).toUpperCase();
}

/* Building blocks */

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function icon(paths) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 20 20');
  svg.setAttribute('aria-hidden', 'true');
  for (const d of paths) {
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', d);
    svg.append(path);
  }
  return svg;
}

function button(label, action) {
  const node = el('button', '', label);
  node.type = 'button';
  node.addEventListener('click', () => run(action));
  return node;
}

/** Runs a command, and says so if it fails. */
async function run(action) {
  try {
    hideProblem();
    return await action();
  } catch (error) {
    showProblem(String(error));
    return undefined;
  }
}

function showProblem(text) {
  const problem = $('problem');
  problem.textContent = text;
  problem.hidden = false;
  clearTimeout(showProblem.timer);
  showProblem.timer = setTimeout(hideProblem, 8000);
}

function hideProblem() {
  $('problem').hidden = true;
}

/* One row per download */

function createRow(item) {
  const r = {
    row: el('article', 'row'),
    badge: el('div', 'badge'),
    name: el('span', 'name'),
    actions: el('div', 'actions'),
    strip: el('div', 'strip'),
    meta: el('div', 'meta'),
    key: null,
    parts: -1,
  };
  const top = el('div', 'top');
  top.append(r.name, r.actions);
  const body = el('div', 'body');
  body.append(top, r.strip, r.meta);
  r.row.append(r.badge, body);
  r.row.addEventListener('dblclick', (event) => {
    if (r.row.dataset.status === 'done' && !event.target.closest('button')) {
      run(() => invoke('open_file', { id: item.id }));
    }
  });
  return r;
}

function updateRow(r, item) {
  r.row.dataset.status = item.status;
  r.badge.textContent = badge(item.name);
  if (r.name.textContent !== item.name) {
    r.name.textContent = item.name;
    r.name.title = `${item.name}\n${item.url}`;
  }
  const key = `${item.status}:${item.resumable}`;
  if (r.key !== key) {
    r.key = key;
    r.actions.replaceChildren(...actionsFor(item));
  }
  drawStrip(r, item);
  r.meta.replaceChildren(...metaFor(item).map(([text, className]) => el('span', className, text)));
}

function actionsFor(item) {
  const id = item.id;
  const buttons = [];
  if (item.status === 'downloading') buttons.push(button('Pause', () => invoke('pause', { id })));
  if (item.status === 'paused') {
    buttons.push(button(item.resumable ? 'Resume' : 'Start again', () => invoke('resume', { id })));
  }
  if (item.status === 'failed') buttons.push(button('Try again', () => invoke('resume', { id })));
  if (item.status === 'done') {
    buttons.push(button('Open', () => invoke('open_file', { id })));
    buttons.push(button(mac ? 'Show in Finder' : 'Show in folder', () => invoke('show_in_folder', { id })));
  }
  buttons.push(removeButton(item));
  return buttons;
}

/** Removing a finished download only takes it off the list. An unfinished
    one loses what it has fetched, so that takes a second click. */
function removeButton(item) {
  const cross = icon(['M5 5l10 10M15 5L5 15']);
  const node = el('button', 'remove');
  node.type = 'button';
  const finished = item.status === 'done';
  const label = finished ? 'Remove from the list (the file stays)' : 'Remove, and delete what it has fetched';
  node.title = label;
  node.setAttribute('aria-label', label);
  node.append(cross);
  let armed = null;
  node.addEventListener('click', () => {
    if (finished || armed) {
      clearTimeout(armed);
      run(() => invoke('remove', { id: item.id }));
      return;
    }
    node.classList.add('confirm');
    node.replaceChildren('Remove?');
    armed = setTimeout(() => {
      armed = null;
      node.classList.remove('confirm');
      node.replaceChildren(cross);
    }, 3000);
  });
  return node;
}

function drawStrip(r, item) {
  let parts = item.parts;
  if (item.status === 'done' && parts.length === 0) parts = [[0, 1, 1]];
  if (parts.length === 0) parts = [[0, 1, item.size ? item.received / item.size : 0]];
  r.strip.classList.toggle('unknown', item.status === 'downloading' && !item.size);
  if (r.parts !== parts.length) {
    // New elements get their sizes before they are shown, so nothing animates from zero.
    r.strip.replaceChildren(...parts.map((part) => sizePart(el('span', 'part'), part, true)));
    r.parts = parts.length;
  } else {
    parts.forEach((part, i) => sizePart(r.strip.children[i], part, false));
  }
}

function sizePart(node, [start, end, upto], fresh) {
  if (fresh) node.append(el('span', 'fill'));
  node.style.left = `${start * 100}%`;
  node.style.width = `${(end - start) * 100}%`;
  const filled = end > start ? Math.min(1, Math.max(0, (upto - start) / (end - start))) : 1;
  node.firstChild.style.width = `${filled * 100}%`;
  node.classList.toggle('complete', filled >= 1);
  return node;
}

function metaFor(item) {
  const of = item.size ? `${size(item.received)} of ${size(item.size)}` : size(item.received);
  switch (item.status) {
    case 'downloading': {
      if (!item.connections && !item.received) return [['Connecting…']];
      const words = [[of]];
      if (item.speed) words.push([`${size(item.speed)}/s`]);
      if (item.eta !== null && item.speed) words.push([`${duration(item.eta)} left`]);
      words.push([item.connections === 1 ? '1 connection' : `${item.connections} connections`]);
      return words;
    }
    case 'paused':
      if (!item.resumable) return [["Paused. This server can't resume, so it starts again from the beginning."]];
      return [['Paused'], [of]];
    case 'failed':
      return [[item.error || 'Stopped by a problem', 'error']];
    default:
      return [[size(item.size ?? item.received)], [`Finished ${finishedAt(item.finished ?? item.added)}`]];
  }
}

/* The whole window */

function render(items) {
  const list = $('list');
  const seen = new Set();
  items.forEach((item, i) => {
    seen.add(item.id);
    let r = rows.get(item.id);
    if (!r) {
      r = createRow(item);
      rows.set(item.id, r);
    }
    updateRow(r, item);
    if (list.children[i] !== r.row) list.insertBefore(r.row, list.children[i] || null);
  });
  for (const [id, r] of rows) {
    if (!seen.has(id)) {
      r.row.remove();
      rows.delete(id);
    }
  }
  $('empty').hidden = items.length > 0;
  $('clear').hidden = !items.some((item) => item.status === 'done');
  const running = items.filter((item) => item.status === 'downloading');
  const speed = running.reduce((total, item) => total + item.speed, 0);
  $('summary').textContent = running.length
    ? `${size(speed)}/s · ${running.length} downloading`
    : items.length
      ? 'Nothing downloading'
      : '';
}

async function add(text) {
  if (!text.trim()) {
    $('link').focus();
    return;
  }
  try {
    hideProblem();
    await invoke('add', { text });
    $('link').value = '';
  } catch (error) {
    showProblem(String(error));
    const bar = $('add');
    bar.classList.remove('shake');
    void bar.offsetWidth;
    bar.classList.add('shake');
  }
}

/* Settings */

function showSettings() {
  $('folder').replaceChildren(el('bdi', '', settings.folder));
  $('folder').title = settings.folder;
  $('parts').textContent = settings.connections;
  for (const choice of $('connections').children) {
    choice.setAttribute('aria-checked', String(Number(choice.dataset.value) === settings.connections));
  }
}

async function openSettings() {
  settings = await invoke('get_settings');
  showSettings();
  $('settings').hidden = false;
  $('settings').querySelector('[data-close]').focus();
}

function closeSettings() {
  $('settings').hidden = true;
  $('open-settings').focus();
}

/* Wiring */

$('add').addEventListener('submit', (event) => {
  event.preventDefault();
  add($('link').value);
});

$('link').addEventListener('input', hideProblem);
$('open-settings').addEventListener('click', openSettings);
$('settings').addEventListener('click', (event) => {
  if (event.target === $('settings') || event.target.closest('[data-close]')) closeSettings();
});
$('change-folder').addEventListener('click', () =>
  run(async () => {
    const chosen = await invoke('choose_folder');
    if (chosen) {
      settings = chosen;
      showSettings();
    }
  }),
);
$('connections').addEventListener('click', (event) => {
  const choice = event.target.closest('[data-value]');
  if (!choice) return;
  run(async () => {
    settings = await invoke('set_connections', { connections: Number(choice.dataset.value) });
    showSettings();
  });
});
$('project').addEventListener('click', () => invoke('open_project'));
$('update').addEventListener('click', () => invoke('open_releases'));
$('clear').addEventListener('click', () => run(() => invoke('clear_finished')));

window.addEventListener('keydown', (event) => {
  const command = mac ? event.metaKey : event.ctrlKey;
  if (command && event.key.toLowerCase() === 'l') {
    event.preventDefault();
    $('link').focus();
    $('link').select();
  } else if (command && event.key === ',') {
    event.preventDefault();
    openSettings();
  } else if (event.key === 'Escape' && !$('settings').hidden) {
    closeSettings();
  }
});

// A link dragged from a browser onto the window downloads, wherever it lands.
let dragDepth = 0;
window.addEventListener('dragenter', (event) => {
  event.preventDefault();
  dragDepth += 1;
  $('drop').hidden = false;
});
window.addEventListener('dragover', (event) => event.preventDefault());
window.addEventListener('dragleave', () => {
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) $('drop').hidden = true;
});
window.addEventListener('drop', (event) => {
  event.preventDefault();
  dragDepth = 0;
  $('drop').hidden = true;
  const data = event.dataTransfer;
  const text = data.getData('text/uri-list') || data.getData('text/plain');
  if (text) add(text);
});

// The browser's own menu has nothing for this window, except in the link field.
document.addEventListener('contextmenu', (event) => {
  if (!event.target.closest('input')) event.preventDefault();
});

$('keys').replaceChildren(
  ...(mac
    ? [el('kbd', '', '⌘'), ' ', el('kbd', '', 'V'), ' then ', el('kbd', '', 'Return')]
    : [el('kbd', '', 'Ctrl'), ' ', el('kbd', '', 'V'), ' then ', el('kbd', '', 'Enter')]),
);

(async () => {
  await listen('downloads', (event) => render(event.payload));
  render(await invoke('list'));
  settings = await invoke('get_settings');
  showSettings();
  $('version').textContent = await invoke('version');
  $('link').focus();
  setTimeout(async () => {
    const update = await invoke('check_update').catch(() => null);
    if (update) {
      $('update').textContent = `indir ${update.version} is out`;
      $('update').hidden = false;
    }
  }, 4000);
})();
