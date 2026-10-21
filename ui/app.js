// indir's window: the link field, and the list the app keeps sending.
'use strict';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const mac = navigator.userAgent.includes('Mac');
const rows = new Map();
let settings = { folder: '', connections: 8, atOnce: 3, speedLimit: null };
let items = [];
/** What happens once every download is done: nothing, sleep, shutdown or quit. */
let whenDone = 'nothing';
/** The links last put in the field from the clipboard, so they aren't put there again. */
let offered = '';

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

/** Says what went wrong, for 8 seconds, or until the next action if it must `stay`. */
function showProblem(text, stay = false) {
  const problem = $('problem');
  problem.textContent = text;
  problem.hidden = false;
  clearTimeout(showProblem.timer);
  if (!stay) showProblem.timer = setTimeout(hideProblem, 8000);
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
    item,
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
  r.row.addEventListener('contextmenu', (event) => {
    event.preventDefault();
    openMenu(rowMenu(r.item), event.clientX, event.clientY);
  });
  return r;
}

function updateRow(r, item) {
  r.item = item;
  r.row.dataset.status = item.status;
  r.badge.textContent = badge(item.name);
  if (r.name.textContent !== item.name) {
    r.name.textContent = item.name;
    r.name.title = `${item.name}\n${item.url}`;
  }
  const key = `${item.status}:${item.resumable}:${item.answered}`;
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
  if (item.status === 'queued') buttons.push(button('Start now', () => invoke('start_now', { id })));
  if (item.status === 'paused') {
    buttons.push(button(startsOver(item) ? 'Start again' : 'Resume', () => invoke('resume', { id })));
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

/** A server that answered without byte ranges: going on means from the beginning. */
function startsOver(item) {
  return item.answered && !item.resumable;
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
    case 'queued':
      return item.received ? [['Waiting for its turn'], [of]] : [['Waiting for its turn']];
    case 'paused':
      if (startsOver(item)) return [["Paused. This server can't resume, so it starts again from the beginning."]];
      return item.received ? [['Paused'], [of]] : [['Paused']];
    case 'failed':
      return [[item.error || 'Stopped by a problem', 'error']];
    default:
      return [[size(item.size ?? item.received)], [`Finished ${finishedAt(item.finished ?? item.added)}`]];
  }
}

/* The whole window */

function render(next) {
  items = next;
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
  const waiting = items.filter((item) => item.status === 'queued');
  const speed = running.reduce((total, item) => total + item.speed, 0);
  const words = [];
  if (running.length) words.push(`${size(speed)}/s`, `${running.length} downloading`);
  if (waiting.length) words.push(`${waiting.length} waiting`);
  if (running.length && settings.speedLimit) words.push(`limit ${settings.speedLimit / 1e6} MB/s`);
  $('summary').textContent = words.length ? words.join(' · ') : items.length ? 'Nothing downloading' : '';
  showAll();
  showWhenDone();
}

/** Pause all while anything runs or waits; resume all once everything has stopped. */
function showAll() {
  const active = items.filter((item) => item.status === 'downloading' || item.status === 'queued').length;
  const stopped = items.filter((item) => item.status === 'paused' || item.status === 'failed').length;
  const all = $('all');
  all.hidden = active ? active + stopped < 2 : stopped < 2;
  all.textContent = active ? 'Pause all' : 'Resume all';
  all.dataset.command = active ? 'pause_all' : 'resume_all';
}

/* When everything is done */

const whenDoneWords = {
  sleep: { button: 'Sleep when done', title: 'Going to sleep', now: 'Sleep now' },
  shutdown: { button: 'Shut down when done', title: 'Shutting down', now: 'Shut down now' },
  quit: { button: 'Quit when done', title: 'Quitting indir', now: 'Quit now' },
};

function showWhenDone() {
  const active = items.some((item) => item.status === 'downloading' || item.status === 'queued');
  const words = whenDoneWords[whenDone];
  $('when-done').hidden = !active && !words;
  $('when-done').classList.toggle('armed', Boolean(words));
  $('when-done-label').textContent = words ? words.button : 'When done…';
}

function showCountdown(countdown) {
  const sheet = $('countdown');
  const words = countdown && whenDoneWords[countdown.action];
  if (!words) {
    const wasOpen = !sheet.hidden;
    sheet.hidden = true;
    if (wasOpen) $('link').focus();
    return;
  }
  $('countdown-title').textContent = words.title;
  $('countdown-now').textContent = words.now;
  const failed = countdown.failed;
  $('countdown-text').textContent = failed
    ? `Nothing is left to download. ${failed === 1 ? 'One download' : `${failed} downloads`} stopped with a problem.`
    : 'Nothing is left to download.';
  $('countdown-seconds').textContent = countdown.seconds;
  // The ring empties as the minute runs out.
  const ring = $('countdown-ring');
  const length = 2 * Math.PI * ring.r.baseVal.value;
  ring.style.strokeDasharray = `${length}`;
  ring.style.strokeDashoffset = `${length * (1 - (countdown.seconds - 1) / 60)}`;
  if (sheet.hidden) {
    closeMenu();
    closeSettings();
    sheet.hidden = false;
    $('countdown-cancel').focus();
  }
}

/* Menus: a download's own, and what happens once everything is done */

/** Opens a menu at (x, y), kept inside the window. Each entry is an
    { label, action } item, a { label, checked, action } choice, a { title },
    or '-' for a line. */
function openMenu(entries, x, y) {
  const menu = $('menu');
  menu.replaceChildren(
    ...entries.map((entry) => {
      if (entry === '-') return el('div', 'menu-line');
      if (entry.title) return el('div', 'menu-title', entry.title);
      const item = el('button', 'menu-item', entry.label);
      item.type = 'button';
      item.setAttribute('role', entry.checked === undefined ? 'menuitem' : 'menuitemradio');
      if (entry.checked !== undefined) item.setAttribute('aria-checked', String(entry.checked));
      item.addEventListener('click', () => {
        closeMenu();
        run(entry.action);
      });
      return item;
    }),
  );
  if (menu.hidden) closeMenu.returnTo = document.activeElement;
  menu.hidden = false;
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, innerWidth - width - 8))}px`;
  menu.style.top = `${y + height > innerHeight - 8 ? Math.max(8, y - height) : y}px`;
  // The menu itself takes the keys; the arrows reach its items.
  menu.focus();
}

function closeMenu() {
  if ($('menu').hidden) return;
  $('menu').hidden = true;
  closeMenu.returnTo?.focus?.();
}

function rowMenu(item) {
  const id = item.id;
  const entries = [];
  if (item.status === 'downloading') entries.push({ label: 'Pause', action: () => invoke('pause', { id }) });
  if (item.status === 'queued') {
    entries.push({ label: 'Start now', action: () => invoke('start_now', { id }) });
    entries.push({ label: 'Pause', action: () => invoke('pause', { id }) });
  }
  if (item.status === 'paused') {
    entries.push({ label: startsOver(item) ? 'Start again' : 'Resume', action: () => invoke('resume', { id }) });
  }
  if (item.status === 'failed') entries.push({ label: 'Try again', action: () => invoke('resume', { id }) });
  if (item.status === 'done') entries.push({ label: 'Open', action: () => invoke('open_file', { id }) });
  entries.push({ label: mac ? 'Show in Finder' : 'Show in folder', action: () => invoke('show_in_folder', { id }) });
  entries.push('-', { label: 'Copy link', action: () => invoke('copy_link', { id }) });
  if (item.status === 'done') entries.push({ label: 'Download again', action: () => invoke('add', { text: item.url }) });
  return entries;
}

const whenDoneChoices = [
  ['nothing', 'Do nothing'],
  ['sleep', 'Sleep'],
  ['shutdown', 'Shut down'],
  ['quit', 'Quit indir'],
];

/** Opens above its button, at the bottom of the window. */
function openWhenDone() {
  const at = $('when-done').getBoundingClientRect();
  const choices = whenDoneChoices.map(([action, label]) => ({
    label,
    checked: whenDone === action,
    action: () => invoke('set_when_done', { action }),
  }));
  openMenu([{ title: 'When the downloads are done' }, ...choices], at.left, at.top - 6);
}

/* Links just copied */

/** A link copied in the browser waits in the field when indir comes to the front. */
async function offerClipboard() {
  const field = $('link');
  const busy = () =>
    field.value.trim() || !$('settings').hidden || !$('countdown').hidden || !$('menu').hidden;
  if (busy()) return;
  const links = await invoke('clipboard_links').catch(() => null);
  // Something may have opened while the clipboard was read.
  if (!links || links === offered || busy()) return;
  offered = links;
  field.value = links;
  field.focus();
  field.select();
  $('from-clipboard').hidden = false;
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
    $('from-clipboard').hidden = true;
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
  mark($('connections'), settings.connections);
  mark($('at-once'), settings.atOnce);
  mark($('limit'), settings.speedLimit);
}

/** Checks the choice whose value is `value`; an empty data-value stands for none. */
function mark(group, value) {
  for (const choice of group.children) {
    choice.setAttribute('aria-checked', String(choice.dataset.value === String(value ?? '')));
  }
}

/** A segmented choice: clicking one sends its value, or null for the empty one. */
function choose(group, command, argument) {
  group.addEventListener('click', (event) => {
    const choice = event.target.closest('[data-value]');
    if (!choice) return;
    const value = choice.dataset.value ? Number(choice.dataset.value) : null;
    run(async () => {
      settings = await invoke(command, { [argument]: value });
      showSettings();
      render(items);
    });
  });
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

$('link').addEventListener('input', () => {
  hideProblem();
  $('from-clipboard').hidden = true;
});
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
choose($('connections'), 'set_connections', 'connections');
choose($('at-once'), 'set_at_once', 'atOnce');
choose($('limit'), 'set_speed_limit', 'limit');
$('project').addEventListener('click', () => invoke('open_project'));
$('update').addEventListener('click', () => invoke('open_releases'));
$('clear').addEventListener('click', () => run(() => invoke('clear_finished')));
$('all').addEventListener('click', () => run(() => invoke($('all').dataset.command)));
$('when-done').addEventListener('click', () => {
  // A second click on the button closes its menu, which the press already did.
  if (closeMenu.closedOn === $('when-done')) return;
  openWhenDone();
});
$('countdown-cancel').addEventListener('click', () => run(() => invoke('cancel_when_done')));
$('countdown-now').addEventListener('click', () => run(() => invoke('when_done_now')));
window.addEventListener('focus', offerClipboard);

// A menu closes on a press anywhere else, and when the window moves on.
document.addEventListener(
  'pointerdown',
  (event) => {
    closeMenu.closedOn = null;
    if ($('menu').hidden || event.target.closest('#menu')) return;
    closeMenu.closedOn = event.target.closest('button');
    closeMenu();
  },
  true,
);
window.addEventListener('blur', closeMenu);
window.addEventListener('resize', closeMenu);
$('list').addEventListener('scroll', closeMenu);
$('menu').addEventListener('keydown', (event) => {
  const choices = [...$('menu').querySelectorAll('button')];
  const at = choices.indexOf(document.activeElement);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    const step = event.key === 'ArrowDown' ? 1 : -1;
    choices[(at + step + choices.length) % choices.length]?.focus();
  } else if (event.key === 'Tab') {
    closeMenu();
  }
});

window.addEventListener('keydown', (event) => {
  const command = mac ? event.metaKey : event.ctrlKey;
  if (command && event.key.toLowerCase() === 'l') {
    event.preventDefault();
    $('link').focus();
    $('link').select();
  } else if (command && event.key === ',') {
    event.preventDefault();
    openSettings();
  } else if (event.key === 'Escape' && !$('menu').hidden) {
    closeMenu();
  } else if (event.key === 'Escape' && !$('countdown').hidden) {
    run(() => invoke('cancel_when_done'));
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
  await listen('when-done', (event) => {
    whenDone = event.payload;
    showWhenDone();
  });
  await listen('countdown', (event) => showCountdown(event.payload));
  // From the app, perhaps while nobody was looking: it stays until the next action.
  await listen('problem', (event) => showProblem(event.payload, true));
  settings = await invoke('get_settings');
  whenDone = await invoke('get_when_done');
  render(await invoke('list'));
  showSettings();
  $('version').textContent = await invoke('version');
  $('link').focus();
  offerClipboard();
  setTimeout(async () => {
    const update = await invoke('check_update').catch(() => null);
    if (update) {
      $('update').textContent = `indir ${update.version} is out`;
      $('update').hidden = false;
    }
  }, 4000);
})();
