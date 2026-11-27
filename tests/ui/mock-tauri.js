// A stand-in for Tauri, so the page runs in a plain browser: tests/ui/preview.html.
// ?fixture=<name> picks the list, ?whenDone=<action> what happens once it's done,
// ?clock=<HH:MM> fixes the time of day (for screenshots).
'use strict';

(function () {
  const params = new URLSearchParams(location.search);

  const clock = params.get('clock');
  if (clock) {
    const [h, m] = clock.split(':').map(Number);
    const fixed = new Date();
    fixed.setHours(h, m, 0, 0);
    const RealDate = Date;
    const at = fixed.getTime();
    // eslint-disable-next-line no-global-assign
    Date = class extends RealDate {
      constructor(...args) {
        super(...(args.length ? args : [at]));
      }
      static now() {
        return at;
      }
    };
  }

  const now = Math.floor(Date.now() / 1000);
  let id = 1;
  /** Parts as [start, end, upto] fractions, each part filled to its share of `fraction`. */
  function parts(count, fraction, jitter = 0.35) {
    return Array.from({ length: count }, (_, i) => {
      const start = i / count;
      const end = (i + 1) / count;
      const wobble = 1 + jitter * Math.sin(i * 2.3 + fraction * 7);
      const share = Math.min(1, Math.max(0, fraction * wobble));
      return [start, end, start + (end - start) * share];
    });
  }

  function item(fields) {
    const base = {
      id: id++,
      url: `https://example.com/files/${fields.name}`,
      folder: '/home/you/Downloads',
      status: 'downloading',
      error: null,
      size: null,
      received: 0,
      speed: 0,
      eta: null,
      resumable: true,
      answered: true,
      connections: 0,
      parts: [],
      added: now - 3600,
      finished: null,
    };
    const it = { ...base, ...fields };
    if (it.status === 'downloading' && it.size && !fields.parts) {
      it.parts = parts(it.connections || 8, it.received / it.size);
    }
    return it;
  }

  const GB = 1e9;
  const MB = 1e6;
  const fixtures = {
    mixed: () => [
      item({ name: 'blender-4.5.3-windows-x64.zip', size: 412 * MB, received: 348 * MB, speed: 9.8 * MB, eta: 7, connections: 6 }),
      item({ name: 'ubuntu-24.04.3-desktop-amd64.iso', size: 6.2 * GB, received: 4.0 * GB, speed: 31.4 * MB, eta: 70, connections: 8 }),
      item({ name: 'lecture-week-6.mp4', status: 'queued', size: 1.2 * GB, received: 520 * MB }),
      item({ name: 'krita-5.2.9-x86_64.AppImage', status: 'done', size: 210 * MB, received: 210 * MB, finished: now - 600 }),
    ],
    'long-name': () => [
      item({
        name: `${'a-really-long-file-name-without-any-spaces-'.repeat(3)}final-v2-release-candidate.tar.gz`,
        size: 900 * MB, received: 300 * MB, speed: 12 * MB, eta: 50, connections: 8,
      }),
      item({ name: 'short.zip', size: 900 * MB, received: 300 * MB, speed: 12 * MB, eta: 50, connections: 8 }),
    ],
    'no-size': () => [item({ name: 'stream.bin', size: null, received: 37 * MB, speed: 4 * MB, connections: 1 })],
    states: () => [
      item({ name: 'connecting.zip', size: null, received: 0, connections: 0 }),
      item({ name: 'downloading.iso', size: 2 * GB, received: 0.9 * GB, speed: 20 * MB, eta: 55, connections: 8 }),
      item({ name: 'queued.mp4', status: 'queued', size: 700 * MB, received: 0 }),
      item({ name: 'paused.flac', status: 'paused', size: 80 * MB, received: 50 * MB }),
      item({ name: 'starts-over.exe', status: 'paused', size: 80 * MB, received: 10 * MB, resumable: false }),
      item({ name: 'failed.pdf', status: 'failed', size: 30 * MB, received: 12 * MB, error: 'The server answered 404: the file is gone.' }),
      item({ name: 'done.AppImage', status: 'done', size: 210 * MB, received: 210 * MB, finished: now - 60 }),
    ],
    fills: () =>
      ['zip', 'iso', 'mp4', 'flac', 'exe', 'pdf', 'bin'].flatMap((ext) =>
        [0, 0.5, 0.995].map((f) =>
          item({ name: `${ext}-at-${Math.round(f * 100)}.${ext}`, size: 100 * MB, received: f * 100 * MB, speed: 5 * MB, eta: 20, connections: 4 }),
        ),
      ),
    many: () =>
      Array.from({ length: 12 }, (_, i) =>
        item({
          name: `file-${i + 1}.${['zip', 'iso', 'mp4', 'flac', 'exe', 'pdf'][i % 6]}`,
          status: i < 3 ? 'downloading' : i < 6 ? 'queued' : 'done',
          size: (i + 1) * 100 * MB,
          received: i < 3 ? (i + 1) * 30 * MB : i < 6 ? 0 : (i + 1) * 100 * MB,
          speed: i < 3 ? 8 * MB : 0,
          eta: i < 3 ? 40 : null,
          connections: i < 3 ? 8 : 0,
          finished: i >= 6 ? now - i * 600 : null,
        }),
      ),
    empty: () => [],
  };

  const items = (fixtures[params.get('fixture')] || fixtures.mixed)();
  const handlers = {};
  let whenDone = params.get('whenDone') || 'nothing';
  const settings = { folder: '/home/you/Downloads', connections: 8, atOnce: 3, speedLimit: null };

  const answers = {
    list: () => items,
    get_settings: () => settings,
    get_when_done: () => whenDone,
    set_when_done: ({ action }) => {
      whenDone = action;
      emit('when-done', action);
    },
    version: () => '0.2.0',
    check_update: () => null,
    clipboard_links: () => null,
  };

  function emit(name, payload) {
    for (const handler of handlers[name] || []) handler({ payload });
  }

  window.__TAURI__ = {
    core: {
      invoke: async (command, args) => {
        window.__mockCalls = [...(window.__mockCalls || []), [command, args]];
        return answers[command] ? answers[command](args || {}) : null;
      },
    },
    event: {
      listen: async (name, handler) => {
        (handlers[name] ||= []).push(handler);
        return () => {};
      },
    },
  };
  window.__mock = { emit, items };
})();
