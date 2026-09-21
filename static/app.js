// Header: hides on scroll down, returns on scroll up; always shown near the top,
// for ~900ms after load, and while the page scrolls itself (follow mode).
const navGrace = (() => {
  const hdr = document.querySelector('.top');
  let graceUntil = performance.now() + 900;
  const grace = ms => { graceUntil = Math.max(graceUntil, performance.now() + ms); };
  if (!hdr) return grace;
  let lastY = window.scrollY;
  window.addEventListener('scroll', () => {
    const y = window.scrollY, delta = y - lastY;
    lastY = y;
    if (y < 96 || performance.now() < graceUntil) { hdr.classList.remove('is-hidden'); return; }
    if (delta > 6) hdr.classList.add('is-hidden');
    else if (delta < -6) hdr.classList.remove('is-hidden');
  }, { passive: true });
  hdr.addEventListener('focusin', () => hdr.classList.remove('is-hidden'));
  return grace;
})();

// Applies server-rendered fragments pushed over SSE. Kinds: block, tool, tool_result, decision, stats.
(() => {
  const tl = document.getElementById('timeline');
  if (!tl) return;
  const sid = tl.dataset.session;
  let seq = parseInt(tl.dataset.seq || '0', 10);
  const live = document.getElementById('live');
  const dec = document.getElementById('decisions');
  const dcount = document.getElementById('dcount');
  const $ = id => document.getElementById(id);

  // View toggles → body classes (CSS does the hiding).
  const toggles = { 't-reasoning': 'hide-reasoning', 't-tools': 'hide-tools', 't-inferred': 'hide-inferred' };
  for (const [id, cls] of Object.entries(toggles)) {
    const el = $(id);
    if (!el) continue;
    const key = 'cp.' + id;
    try { const saved = localStorage.getItem(key); if (saved !== null) el.checked = saved === '1'; } catch {}
    const apply = () => {
      document.body.classList.toggle(cls, !el.checked);
      try { localStorage.setItem(key, el.checked ? '1' : '0'); } catch {}
    };
    el.addEventListener('change', apply);
    apply();
  }
  const following = () => $('t-follow')?.checked;

  // Small screens: decisions start collapsed so the timeline is reachable.
  const dpanel = $('dpanel');
  if (dpanel && window.matchMedia('(max-width: 1000px)').matches) dpanel.removeAttribute('open');

  const frag = html => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content; };

  function apply(m) {
    switch (m.kind) {
      case 'tool': {
        const step = m.step && $(m.step)?.querySelector('.tools');
        (step || tl).appendChild(frag(m.html));
        break;
      }
      case 'tool_result': {
        const li = $('tool-' + m.tool);
        if (!li) return;
        const slot = li.querySelector('[data-slot]');
        if (slot) slot.innerHTML = m.html;
        const rd = li.querySelector('[data-rslot]');
        if (rd) rd.textContent = m.detail || '';
        li.classList.add(m.ok ? 'ok' : 'err');
        break;
      }
      case 'decision':
        tl.appendChild(frag(m.html));
        dec.appendChild(frag(m.panel));
        break;
      case 'stats': {
        if (m.prompt) { const c = $('chips-' + m.prompt); if (c && m.chips != null) c.innerHTML = m.chips; }
        const h = $('hdr-stats'); if (h) h.innerHTML = m.header;
        if (dcount && m.dcount != null) dcount.textContent = m.dcount;
        return;
      }
      default:
        tl.appendChild(frag(m.html));
    }
    if (following()) {
      navGrace(1200);
      window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'smooth' });
    }
  }

  // EventSource reconnects on its own and sends Last-Event-ID, which the server honours over `from`.
  const es = new EventSource(`/s/${sid}/events?from=${seq}`);
  es.onopen = () => live.classList.add('on');
  es.onerror = () => live.classList.remove('on');
  es.onmessage = e => {
    let m; try { m = JSON.parse(e.data); } catch { return; }
    apply(m);
    if (e.lastEventId) seq = parseInt(e.lastEventId, 10) + 1;
  };
})();
