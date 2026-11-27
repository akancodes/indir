// The mini strip: every download together, as one card filling up. A click brings
// the main window back; a drag moves the strip.
'use strict';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { nextBatch, batchView, size, duration, shortcut } = window.indirLogic;

const $ = (id) => document.getElementById(id);
const mac = navigator.userAgent.includes('Mac');
let batch = null;

function draw(items) {
  batch = nextBatch(batch, items);
  const view = batchView(batch, items);
  const strip = $('strip');
  const idle = view.running + view.waiting === 0;
  strip.classList.toggle('idle', idle);
  strip.classList.toggle('unknown', !idle && view.fraction === null);
  strip.classList.toggle('problem', view.problem);
  $('mini-fill').style.width = `${idle ? 0 : (view.fraction ?? 0) * 100}%`;
  $('mini-percent').textContent = idle || view.fraction === null ? '' : `${Math.floor(view.fraction * 100)}%`;
  if (idle) {
    $('mini-speed').textContent = 'Nothing downloading';
    $('mini-count').textContent = '';
    return;
  }
  const speed = [];
  if (view.speed) speed.push(`${size(view.speed)}/s`);
  if (view.eta !== null) speed.push(`${duration(view.eta)} left`);
  $('mini-speed').textContent = speed.length ? speed.join(' · ') : 'Starting…';
  const count = [];
  if (view.running) count.push(`${view.running} downloading`);
  if (view.waiting) count.push(`${view.waiting} waiting`);
  $('mini-count').textContent = count.join(' · ');
}

// A press that moves is a drag; one that doesn't is a click. Mouse events, not pointer
// events: once the window manager has taken a drag, the page never hears that button
// come up, so the next press brings no pointerdown, only a mousedown.
let pressed = null;
$('strip').addEventListener('mousedown', (event) => {
  if (event.button === 0) pressed = { x: event.screenX, y: event.screenY };
});
$('strip').addEventListener('mousemove', (event) => {
  if (!pressed) return;
  if (Math.hypot(event.screenX - pressed.x, event.screenY - pressed.y) > 4) {
    pressed = null;
    invoke('drag_mini');
  }
});
$('strip').addEventListener('mouseup', () => {
  if (pressed) invoke('show_main');
  pressed = null;
});
$('strip').addEventListener('mouseleave', () => {
  pressed = null;
});

window.addEventListener('keydown', (event) => {
  if (shortcut(event, mac) === 'mini' || event.key === 'Enter' || event.key === 'Escape') {
    event.preventDefault();
    invoke('show_main');
  }
});

document.addEventListener('contextmenu', (event) => event.preventDefault());

(async () => {
  await listen('downloads', (event) => draw(event.payload));
  draw(await invoke('list'));
})();
