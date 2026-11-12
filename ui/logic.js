// What the window shows, worked out without a window: numbers in words, the colour a file
// gets, how full its card is, and the mini strip's batch. A plain script for the page
// (window.indirLogic) and a CommonJS module for the tests.
'use strict';

(function (root) {
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

  const groups = {
    archive: ['zip', '7z', 'rar', 'tar', 'gz', 'tgz', 'xz', 'bz2', 'zst'],
    disc: ['iso', 'img', 'dmg'],
    video: ['mp4', 'mkv', 'mov', 'webm', 'avi', 'm4v'],
    audio: ['mp3', 'flac', 'm4a', 'wav', 'ogg', 'opus', 'aac'],
    app: ['exe', 'msi', 'appimage', 'deb', 'rpm', 'pkg', 'apk', 'msix'],
    document: ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'epub', 'txt', 'odt'],
  };
  const groupOf = new Map(Object.entries(groups).flatMap(([group, list]) => list.map((ext) => [ext, group])));

  /** archive, disc, video, audio, app, document or other, by the name's extension. */
  function typeGroup(name) {
    const dot = name.lastIndexOf('.');
    if (dot <= 0) return 'other';
    return groupOf.get(name.slice(dot + 1).toLowerCase()) || 'other';
  }

  /** How full a card is, 0 to 1, or null while the size is unknown. */
  function fillFraction(item) {
    if (item.status === 'done') return 1;
    if (!item.size) return null;
    return Math.min(1, Math.max(0, item.received / item.size));
  }

  /** A server that answered without byte ranges: going on means from the beginning. */
  function startsOver(item) {
    return item.answered && !item.resumable;
  }

  /** The line under a card's name. */
  function cardWords(item) {
    const of = item.size ? `${size(item.received)} of ${size(item.size)}` : size(item.received);
    switch (item.status) {
      case 'downloading': {
        if (!item.connections && !item.received) return { main: ['Connecting…'], error: false };
        const main = [of];
        if (item.speed) main.push(`${size(item.speed)}/s`);
        if (item.eta !== null && item.speed) main.push(`${duration(item.eta)} left`);
        return { main, error: false };
      }
      case 'queued':
        return { main: item.received ? ['Waiting its turn', of] : ['Waiting its turn'], error: false };
      case 'paused':
        if (startsOver(item)) {
          return { main: ["Paused. This server can't resume, so it starts again from the beginning."], error: false };
        }
        return { main: item.received ? ['Paused', of] : ['Paused'], error: false };
      case 'failed':
        return { main: [item.error || 'Stopped by a problem'], error: true };
      default:
        return {
          main: [size(item.size ?? item.received), `done ${finishedAt(item.finished ?? item.added)}`],
          error: false,
        };
    }
  }

  const thenWords = { sleep: 'then sleep', shutdown: 'then shut down', quit: 'then quit' };

  /** The footer's line: speed and counts, and what happens once everything is done. */
  function summary(items, speedLimit, whenDone) {
    const running = items.filter((item) => item.status === 'downloading');
    const waiting = items.filter((item) => item.status === 'queued');
    const speed = running.reduce((total, item) => total + item.speed, 0);
    const words = [];
    if (running.length) words.push(`${size(speed)}/s`, `${running.length} downloading`);
    if (waiting.length) words.push(`${waiting.length} waiting`);
    if (running.length && speedLimit) words.push(`limit ${speedLimit / 1e6} MB/s`);
    if (!words.length) return items.length ? 'Nothing downloading' : '';
    if (thenWords[whenDone]) words.push(thenWords[whenDone]);
    return words.join(' · ');
  }

  const isActive = (item) => item.status === 'downloading' || item.status === 'queued';

  function emptyBatch() {
    return { ids: new Set(), sizes: new Map(), got: new Map(), shown: 0 };
  }

  /** The strip counts a batch: from the first download after an idle spell until nothing
      downloads or waits. What joined it stays counted, finished or removed, and what it
      shows only ever grows. */
  function nextBatch(batch, items) {
    if (!items.some(isActive)) return emptyBatch();
    const next = batch && batch.ids.size ? batch : emptyBatch();
    for (const item of items) {
      if (isActive(item)) next.ids.add(item.id);
      if (!next.ids.has(item.id)) continue;
      if (item.size) next.sizes.set(item.id, item.size);
      next.got.set(item.id, item.status === 'done' && item.size ? item.size : item.received);
    }
    let total = 0;
    let got = 0;
    for (const [id, bytes] of next.sizes) {
      total += bytes;
      got += Math.min(bytes, next.got.get(id) || 0);
    }
    if (total) next.shown = Math.max(next.shown, Math.min(1, got / total));
    next.total = total;
    next.left = total - got;
    return next;
  }

  /** What the strip shows for a batch. */
  function batchView(batch, items) {
    const inBatch = items.filter((item) => batch.ids.has(item.id));
    const running = inBatch.filter((item) => item.status === 'downloading');
    const speed = running.reduce((total, item) => total + item.speed, 0);
    return {
      fraction: batch.total ? batch.shown : null,
      speed,
      eta: batch.total && speed ? batch.left / speed : null,
      running: running.length,
      waiting: inBatch.filter((item) => item.status === 'queued').length,
      problem: inBatch.some((item) => item.status === 'failed'),
    };
  }

  const api = {
    size,
    duration,
    finishedAt,
    badge,
    typeGroup,
    fillFraction,
    startsOver,
    cardWords,
    summary,
    nextBatch,
    batchView,
  };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.indirLogic = api;
})(this);
