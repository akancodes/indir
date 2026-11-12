// Takes the real page's markup and runs its scripts, in order, against the stand-in.
'use strict';

(async function () {
  const params = new URLSearchParams(location.search);
  const page = params.get('page') === 'mini' ? 'mini' : 'index';
  // Fresh files each time, so an edit shows on the next load.
  const fresh = `?v=${Date.now()}`;
  const html = await (await fetch(`../../ui/${page}.html`, { cache: 'no-store' })).text();
  const doc = new DOMParser().parseFromString(html, 'text/html');
  for (const link of doc.querySelectorAll('link[rel="stylesheet"]')) {
    const node = document.createElement('link');
    node.rel = 'stylesheet';
    node.href = `../../ui/${link.getAttribute('href')}${fresh}`;
    document.head.append(node);
  }
  document.documentElement.className = doc.documentElement.className;
  document.body.className = doc.body.className;
  document.body.innerHTML = doc.body.innerHTML;
  for (const script of doc.querySelectorAll('script[src]')) {
    await new Promise((resolve, reject) => {
      const node = document.createElement('script');
      node.src = `../../ui/${script.getAttribute('src')}${fresh}`;
      node.onload = resolve;
      node.onerror = reject;
      document.body.append(node);
    });
  }
})();
