// The page's logic, without a window: node --test tests/ui/
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const logic = require('../../ui/logic.js');

let nextId = 1;
function item(fields) {
  return {
    id: nextId++,
    url: 'https://example.com/file',
    name: 'file.bin',
    folder: '/downloads',
    status: 'downloading',
    error: null,
    size: 100,
    received: 0,
    speed: 0,
    eta: null,
    resumable: true,
    answered: true,
    connections: 0,
    parts: [],
    added: 1_700_000_000,
    finished: null,
    ...fields,
  };
}

test('typeGroup sorts names by what they hold', () => {
  const cases = {
    'a.ZIP': 'archive',
    'x.tar.gz': 'archive',
    'x.tgz': 'archive',
    'u.iso': 'disc',
    'm.MKV': 'video',
    's.flac': 'audio',
    'k.AppImage': 'app',
    'setup.exe': 'app',
    'r.pdf': 'document',
    noext: 'other',
    '.bashrc': 'other',
  };
  for (const [name, group] of Object.entries(cases)) assert.equal(logic.typeGroup(name), group, name);
});

test('fillFraction is how far a download has come, or null when unknown', () => {
  assert.equal(logic.fillFraction(item({ status: 'done', size: null, received: 5 })), 1);
  assert.equal(logic.fillFraction(item({ size: 100, received: 50 })), 0.5);
  assert.equal(logic.fillFraction(item({ size: 100, received: 150 })), 1);
  assert.equal(logic.fillFraction(item({ size: null, received: 50 })), null);
  assert.equal(logic.fillFraction(item({ size: 0, received: 0 })), null);
});

test('cardWords leave connections to the details', () => {
  const words = logic.cardWords(item({ size: 412e6, received: 348e6, speed: 9.8e6, eta: 7, connections: 6 }));
  assert.deepEqual(words, { main: ['348 MB of 412 MB', '9.8 MB/s', '7 s left'], error: false });
  assert.ok(!words.main.some((word) => word.includes('connection')));
});

test('cardWords before the first bytes say it is connecting', () => {
  assert.deepEqual(logic.cardWords(item({ received: 0, connections: 0 })).main, ['Connecting…']);
});

test('cardWords for a queued download say it waits its turn', () => {
  assert.deepEqual(logic.cardWords(item({ status: 'queued', received: 0 })).main, ['Waiting its turn']);
  assert.deepEqual(logic.cardWords(item({ status: 'queued', received: 50 })).main, ['Waiting its turn', '50 B of 100 B']);
});

test('cardWords for a failed download are its problem', () => {
  assert.deepEqual(logic.cardWords(item({ status: 'failed', error: '404' })), { main: ['404'], error: true });
  assert.deepEqual(logic.cardWords(item({ status: 'failed' })).main, ['Stopped by a problem']);
});

test('cardWords for a finished download give its size and when', () => {
  const words = logic.cardWords(item({ status: 'done', size: 210e6, received: 210e6, finished: 1_700_000_000 }));
  assert.equal(words.main[0], '210 MB');
  assert.match(words.main[1], /^done /);
});

test('summary adds what happens when done', () => {
  const items = [
    item({ speed: 1e6 }),
    item({ speed: 2e6 }),
    item({ status: 'queued' }),
  ];
  assert.equal(logic.summary(items, null, 'shutdown'), '3.0 MB/s · 2 downloading · 1 waiting · then shut down');
  assert.equal(logic.summary(items, null, 'nothing'), '3.0 MB/s · 2 downloading · 1 waiting');
});

test('summary with nothing active says so, whatever comes after', () => {
  assert.equal(logic.summary([item({ status: 'done' })], null, 'sleep'), 'Nothing downloading');
  assert.equal(logic.summary([], null, 'nothing'), '');
});

/** Runs the strip's batch over a series of lists, as the app sends them. */
function batchOver(...lists) {
  let batch = null;
  return lists.map((items) => {
    batch = logic.nextBatch(batch, items);
    return logic.batchView(batch, items);
  });
}

test('batch: a finished download stays counted, so progress never falls', () => {
  const a = item({ size: 100, received: 0 });
  const b = item({ size: 300, received: 0 });
  const views = batchOver(
    [{ ...a, received: 50 }, { ...b, received: 30 }],
    [{ ...a, status: 'done', received: 100 }, { ...b, received: 60 }],
  );
  assert.equal(views[0].fraction, 80 / 400);
  assert.equal(views[1].fraction, 160 / 400);
});

test('batch: a removed download keeps what it added', () => {
  const a = item({ size: 100 });
  const b = item({ size: 100 });
  const views = batchOver(
    [{ ...a, received: 50 }, { ...b, received: 20 }],
    [{ ...a, received: 100, status: 'done' }, { ...b, received: 20 }],
    [{ ...b, received: 40 }],
  );
  assert.equal(views[0].fraction, 70 / 200);
  assert.equal(views[1].fraction, 120 / 200);
  assert.equal(views[2].fraction, 140 / 200);
});

test('batch: a size learned late never makes progress fall', () => {
  const a = item({ size: 100 });
  const b = item({ size: null, status: 'queued' });
  const views = batchOver(
    [{ ...a, received: 80 }, b],
    [{ ...a, received: 90 }, { ...b, status: 'downloading', size: 900, received: 10 }],
  );
  assert.equal(views[0].fraction, 0.8);
  assert.ok(views[1].fraction >= 0.8);
});

test('batch: fraction never goes over 1', () => {
  const a = item({ size: 100 });
  const [view] = batchOver([{ ...a, received: 250 }]);
  assert.equal(view.fraction, 1);
});

test('batch: sizes nobody knows add nothing', () => {
  const [view] = batchOver([item({ size: null, received: 500 })]);
  assert.equal(view.fraction, null);
  assert.equal(view.running, 1);
});

test('batch: an idle spell ends the batch, and the next download starts a fresh one', () => {
  const a = item({ size: 100 });
  const b = item({ size: 100 });
  const views = batchOver(
    [{ ...a, received: 100, status: 'done' }],
    [{ ...a, received: 100, status: 'done' }, { ...b, received: 10 }],
  );
  assert.equal(views[0].running + views[0].waiting, 0);
  assert.equal(views[1].fraction, 0.1);
});

test('batch: speed, time left and counts come from the downloads in it', () => {
  const [view] = batchOver([
    item({ size: 100, received: 0, speed: 10 }),
    item({ size: 100, received: 0, speed: 30 }),
    item({ status: 'queued', size: 100 }),
  ]);
  assert.equal(view.speed, 40);
  assert.equal(view.eta, 300 / 40);
  assert.equal(view.running, 2);
  assert.equal(view.waiting, 1);
});

test('batch: a download that stops with a problem is shown', () => {
  const a = item({ size: 100 });
  const b = item({ size: 100 });
  const views = batchOver([a, b], [{ ...a, status: 'failed', error: 'x' }, b]);
  assert.equal(views[0].problem, false);
  assert.equal(views[1].problem, true);
});

test('batch: a lone download that fails keeps the strip red once nothing is left', () => {
  const a = item({ size: 100 });
  const views = batchOver([{ ...a, received: 10 }], [{ ...a, status: 'failed', error: 'x', received: 10 }]);
  assert.equal(views[1].problem, true);
});

test('batch: the red goes once the failed download is tried again or removed', () => {
  const a = item({ size: 100 });
  const views = batchOver([{ ...a, received: 10 }], [{ ...a, status: 'failed', error: 'x' }], []);
  assert.equal(views[2].problem, false);
});

test('batch: an unfinished download removed mid-batch counts as settled, so the rest still moves', () => {
  const a = item({ size: 1000 });
  const b = item({ size: 100 });
  const views = batchOver(
    [{ ...a, received: 100 }, { ...b, received: 10, speed: 100 }],
    [{ ...b, received: 99, speed: 100 }],
  );
  assert.equal(views[1].fraction, 1099 / 1100);
  assert.ok(views[1].eta < 1, `eta ${views[1].eta}`);
});

test('batch: time left counts only what is still coming', () => {
  const a = item({ size: 1000 });
  const b = item({ size: 100 });
  const views = batchOver(
    [{ ...a, received: 100, speed: 50 }, { ...b, received: 50, speed: 50 }],
    [{ ...a, status: 'paused', received: 100 }, { ...b, received: 50, speed: 50 }],
  );
  assert.equal(views[1].eta, 1);
});

test('shortcut: Ctrl+M elsewhere, Shift+Cmd+M on a Mac, where Cmd+M minimizes', () => {
  const key = (k, mods = {}) => ({ key: k, ctrlKey: false, metaKey: false, shiftKey: false, ...mods });
  assert.equal(logic.shortcut(key('m', { ctrlKey: true }), false), 'mini');
  assert.equal(logic.shortcut(key('M', { metaKey: true, shiftKey: true }), true), 'mini');
  assert.equal(logic.shortcut(key('m', { metaKey: true }), true), null);
  assert.equal(logic.shortcut(key('l', { ctrlKey: true }), false), 'link');
  assert.equal(logic.shortcut(key(',', { metaKey: true }), true), 'settings');
  assert.equal(logic.shortcut(key('m'), false), null);
});

test('detailWords hold the whole problem, which the card line may cut', () => {
  const long = 'The server answered 403: you need to be signed in to download this file, try your browser.';
  const words = logic.detailWords(item({ status: 'failed', error: long }));
  assert.ok(words.includes(long));
  const over = logic.detailWords(item({ status: 'paused', answered: true, resumable: false }));
  assert.ok(over.some((word) => word.includes('starts again from the beginning')));
});

test('detailWords: connections while it downloads, then the link and the folder', () => {
  const words = logic.detailWords(item({ connections: 6, url: 'https://x/y.zip', folder: '/d' }));
  assert.deepEqual(words, ['6 connections', 'https://x/y.zip', '/d']);
});
