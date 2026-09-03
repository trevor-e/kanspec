// kanspec board — vanilla JS, no build step. Owner: S8.
//
// One data source: GET /api/board returns the SAME `BoardModel` `kanspec board` renders,
// so the terminal and this page cannot disagree about a column's contents. The SSE stream
// carries {rev, n} and nothing else (D-23) — every tick is "refetch", because macOS
// FSEvents coalesces unpredictably and any event-derived delta would be a bug farm.
//
// Dragging a card POSTs the corresponding verb to the same `cmd::*` function the CLI
// calls. Illegal moves bounce with the tool's own typed refusal, fix line included.

'use strict';

const $ = (id) => document.getElementById(id);

const state = {
  review: null,
  board: null,
  rules: null,       // fetched lazily, the first time the Rules tab is shown
  rulesStale: true,  // …and again whenever the board has moved under it
  tab: localStorage.getItem('ks.tab') || 'all',
  open: null,        // the ticket id showing in the drawer
  fetching: false,
  pending: false,
};

// ── tiny DOM helpers ────────────────────────────────────────────────────────

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined && text !== null) n.textContent = String(text);
  return n;
}

function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

const GLYPH = { todo: '○', doing: '◐', review: '◈', done: '●', dropped: '✕' };
const IN_MAIN = '⇂';
const DISCOVERED = '◇';
const DOT = { ok: '●', stale: '⚠', dead_globs: '◌', never_scanned: '·' };

function age(secs) {
  if (secs === null || secs === undefined) return '—';
  if (secs < 60) return secs + 's';
  if (secs < 3600) return Math.floor(secs / 60) + 'm';
  if (secs < 86400) return Math.floor(secs / 3600) + 'h';
  return Math.floor(secs / 86400) + 'd';
}

function shortPath(p) {
  if (!p) return '—';
  const parts = p.split('/').filter(Boolean);
  return parts.length <= 2 ? p : '…/' + parts.slice(-2).join('/');
}

// ── toasts: the one place a refusal is shown, fix line and all ──────────────

function toast(message, kind, fix) {
  const t = el('div', 'toast ' + (kind || ''));
  t.appendChild(el('div', null, message));
  if (fix) {
    const f = el('code', 'fixline', fix);
    f.title = 'click to copy';
    f.style.cursor = 'pointer';
    f.onclick = () => copy(fix);
    t.appendChild(f);
  }
  $('toasts').appendChild(t);
  setTimeout(() => t.remove(), kind === 'err' ? 9000 : 4000);
}

function copy(text) {
  if (navigator.clipboard) {
    navigator.clipboard.writeText(text).then(
      () => toast('copied: ' + text, 'ok'),
      () => toast(text)
    );
  } else {
    toast(text);
  }
}

// ── the network ─────────────────────────────────────────────────────────────

async function refresh() {
  // On `/p/<id>` the live signal means "the proposal or its threads moved", not "the board
  // did" — refetching the board there would leave the page a snapshot of the moment it
  // loaded while claiming to be live.
  if (state.review) return loadReview(state.review.id);
  if (state.fetching) { state.pending = true; return; }
  state.fetching = true;
  try {
    const res = await fetch('/api/board', { headers: { accept: 'application/json' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    state.board = await res.json();
    state.rulesStale = true;
    render();
  } catch (e) {
    setLive('off', 'unreachable');
    toast('cannot reach the board: ' + e.message, 'err', 'kanspec board');
  } finally {
    state.fetching = false;
    if (state.pending) { state.pending = false; refresh(); }
  }
}

// Every verb the page invokes goes through the SAME handler the CLI calls, so a POST and
// `kanspec <verb>` write byte-identical files. A refusal comes back as the CLI's own
// `--json` error envelope: {ok:false, error:{kind,message,fix,…}}.
async function verb(id, name, body) {
  try {
    const res = await fetch('/api/ticket/' + encodeURIComponent(id) + '/' + name, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body || {}),
    });
    const payload = await res.json().catch(() => null);
    if (!res.ok) {
      const err = (payload && payload.error) || {};
      const fix = err.fix && err.fix.length ? err.fix[0] : null;
      toast(err.message || ('the verb was refused (HTTP ' + res.status + ')'), 'err', fix);
      return false;
    }
    toast(name + ' ' + id, 'ok');
    await refresh();
    if (state.open === id) await openTicket(id);
    return true;
  } catch (e) {
    toast('the verb did not reach the server: ' + e.message, 'err');
    return false;
  }
}

function setLive(cls, text) {
  const n = $('live');
  n.className = 'live ' + cls;
  $('live-text').textContent = text;
}

function subscribe() {
  const es = new EventSource('/events');
  es.onopen = () => setLive('on', 'live');
  es.onmessage = () => refresh();
  es.addEventListener('tick', () => refresh());
  es.onerror = () => {
    setLive('off', 'reconnecting');
    // EventSource retries on its own; the board still refetches on focus.
  };
}

// ── render ──────────────────────────────────────────────────────────────────

function render() {
  const b = state.board;
  if (!b) return;

  // Every badge on this page came out of the cache, so the cache's own age is part of the
  // answer: a merge state nobody has refreshed today has to say so rather than look current.
  const scanned = b.cache_age_secs !== null && b.cache_age_secs !== undefined;
  $('cache-age').textContent = scanned
    ? 'merge state ' + age(b.cache_age_secs) + ' old'
    : 'never scanned';
  $('cache-age').className = 'pill' + (scanned ? '' : ' warn');

  renderAttention(b.attention || []);
  renderFeatures(b.features || []);

  const main = $('main');
  clear(main);
  if (state.tab === 'all') main.appendChild(renderBoard(b.columns));
  else if (state.tab === 'spec') main.appendChild(renderBySpec(b));
  else if (state.tab === 'worktrees') main.appendChild(renderWorktrees(b.worktrees || []));
  else if (state.tab === 'review') main.appendChild(renderReviewQueue(b));
  else main.appendChild(renderRules());

  for (const t of document.querySelectorAll('.tab')) {
    t.setAttribute('aria-selected', String(t.dataset.tab === state.tab));
  }
}

function renderAttention(items) {
  const strip = $('attention');
  clear(strip);
  strip.hidden = items.length === 0;
  if (!items.length) return;

  for (const owner of ['you', 'agent', 'watching']) {
    const group = items.filter((i) => i.owner === owner);
    if (!group.length) continue;
    const row = el('div', 'owner-group');
    row.appendChild(el('div', 'owner-label ' + owner, owner.toUpperCase() + ' (' + group.length + ')'));
    const list = el('div');
    list.style.flex = '1';
    for (const a of group) {
      const line = el('div', 'att');
      line.appendChild(el('span', 'g', a.glyph));
      if (a.subject) {
        const s = el('span', 'subj', a.subject);
        s.style.cursor = 'pointer';
        s.onclick = () => openTicket(a.subject);
        line.appendChild(s);
      }
      line.appendChild(el('span', 'txt', a.line));
      if (a.fix) {
        const f = el('button', 'fix', a.fix);
        f.title = 'copy this command';
        f.onclick = () => copy(a.fix);
        line.appendChild(f);
      }
      list.appendChild(line);
    }
    row.appendChild(list);
    strip.appendChild(row);
  }
}

function renderFeatures(features) {
  const strip = $('features');
  clear(strip);
  strip.hidden = features.length === 0;
  for (const f of features) {
    const kind = f.staleness.staleness;
    const chip = el('span', 'feat ' + kind);
    chip.appendChild(el('span', 'dot', DOT[kind] || '·'));
    chip.appendChild(el('span', 'nm', f.spec));
    chip.appendChild(el('span', 'note', f.note));
    chip.title = f.feature;
    strip.appendChild(chip);
  }
}

function renderBoard(columns) {
  const wrap = el('div', 'board');
  for (const c of columns) wrap.appendChild(renderColumn(c));
  return wrap;
}

function renderColumn(c, cards) {
  const list = cards === undefined ? c.cards : cards;
  const col = el('div', 'col');
  col.dataset.column = c.column;

  const head = el('div', 'col-head');
  head.appendChild(el('span', null, c.title));
  head.appendChild(el('span', 'n', list.length));
  col.appendChild(head);

  if (!list.length) col.appendChild(el('div', 'col-empty', '—'));
  for (const card of list) col.appendChild(renderCard(card));

  // Dragging a card IS the state machine: the drop maps to a verb, and an illegal move
  // bounces with the typed reason rather than being hidden by a disabled drop target.
  col.addEventListener('dragover', (e) => {
    if (!VERB_FOR_COLUMN[c.column]) { col.classList.add('drop-no'); return; }
    e.preventDefault();
    col.classList.add('drop-ok');
  });
  col.addEventListener('dragleave', () => col.classList.remove('drop-ok', 'drop-no'));
  col.addEventListener('drop', (e) => {
    e.preventDefault();
    col.classList.remove('drop-ok', 'drop-no');
    const id = e.dataTransfer.getData('text/plain');
    if (id) dropOnto(id, c.column);
  });
  return col;
}

function renderCard(card) {
  const inMain = card.badge.badge === 'in_main' && card.state !== 'done' && card.state !== 'dropped';
  const cls = ['card', 's-' + card.state];
  if (inMain) cls.push('in-main');
  if (card.stalled_secs !== null && card.stalled_secs !== undefined) cls.push('stalled');

  const n = el('div', cls.join(' '));
  n.draggable = true;
  n.dataset.id = card.id;
  n.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData('text/plain', card.id);
    e.dataTransfer.effectAllowed = 'move';
    n.classList.add('dragging');
  });
  n.addEventListener('dragend', () => n.classList.remove('dragging'));
  n.onclick = () => openTicket(card.id);

  const top = el('div', 'card-top');
  top.appendChild(el('span', 'g', inMain ? IN_MAIN : (GLYPH[card.state] || '?')));
  top.appendChild(el('span', 'id', card.id));
  n.appendChild(top);

  n.appendChild(el('div', 'title', card.title));

  const chips = el('div', 'chips');
  if (card.discovered_in) {
    chips.appendChild(el('span', 'chip disc', DISCOVERED + ' ' + card.discovered_in));
  }
  if (card.spec) chips.appendChild(el('span', 'chip spec', card.spec));
  if (card.proposal) chips.appendChild(el('span', 'chip proposal', card.proposal));
  if (card.branch) chips.appendChild(el('span', 'chip mono', '⎇ ' + card.branch));
  if (card.worktree) {
    const w = el('span', 'chip mono', '⌂ ' + shortPath(card.worktree));
    w.title = card.worktree;
    chips.appendChild(w);
  }
  if (card.blocked_by && card.blocked_by.length) {
    chips.appendChild(el('span', 'chip blocked', 'blocked by ' + card.blocked_by.join(', ')));
  }
  if (card.unresolved > 0) {
    chips.appendChild(el('span', 'chip threads', card.unresolved + ' threads open'));
  }
  if (chips.childNodes.length) n.appendChild(chips);

  const foot = el('div', 'card-foot');
  // The merge badge is never a guess: it is exactly one of the six shapes the tool
  // computed, with its own freshness stamp baked in.
  //
  // The badge is ellipsised at the card's width (see `.badge` in style.css), so the
  // reason — the whole point of an `unknown (…)` — carries a title too: a truncated
  // badge that cannot be read is a badge that says nothing.
  const badge = el('span', 'badge ' + card.badge.badge, card.badge_text);
  badge.title = card.badge_text;
  foot.appendChild(badge);
  if (card.stalled_secs !== null && card.stalled_secs !== undefined) {
    foot.appendChild(el('span', 'flag', 'STALLED ' + age(card.stalled_secs)));
  }
  if (card.claimed_by) foot.appendChild(el('span', 'agent', card.claimed_by));
  foot.appendChild(el('span', 'age', age(card.updated_secs)));
  n.appendChild(foot);
  return n;
}

// ── by spec, with the "unspecced" shame lane ────────────────────────────────

function renderBySpec(b) {
  const wrap = el('div');
  const specs = new Map();
  for (const f of b.features) specs.set(f.spec, { feature: f.feature, staleness: f.staleness, note: f.note });

  // A card can name a spec that has no file yet; the lane still has to exist, or the work
  // becomes invisible.
  for (const c of b.columns) for (const card of c.cards) {
    if (card.spec && !specs.has(card.spec)) specs.set(card.spec, { feature: '(no spec file)', staleness: { staleness: 'never_scanned' }, note: 'no spec file' });
  }

  const lanes = [];
  for (const [name, meta] of specs) lanes.push({ name, meta, match: (c) => c.spec === name });
  lanes.sort((a, x) => a.name.localeCompare(x.name));
  lanes.push({ name: null, meta: null, match: (c) => !c.spec });

  for (const lane of lanes) {
    const cols = b.columns.map((c) => ({ col: c, cards: c.cards.filter(lane.match) }));
    const total = cols.reduce((n, c) => n + c.cards.length, 0);
    // An empty capability lane is noise; the unspecced lane appears only when it is not
    // empty either — but when it IS, it is impossible to miss.
    if (!total) continue;

    const node = el('div', 'lane' + (lane.name === null ? ' unspecced' : ''));
    const head = el('div', 'lane-head');
    head.appendChild(el('span', 'nm', lane.name === null ? 'unspecced' : lane.name));
    if (lane.name === null) {
      head.appendChild(el('span', 'shame', 'these tickets belong to no capability — give them a spec'));
    } else {
      head.appendChild(el('span', 'feature', lane.meta.feature));
      const kind = lane.meta.staleness.staleness;
      const dot = el('span', 'feat ' + kind);
      dot.appendChild(el('span', 'dot', DOT[kind] || '·'));
      dot.appendChild(el('span', 'note', lane.meta.note || kind));
      head.appendChild(dot);
    }
    head.appendChild(el('span', 'n', total + (total === 1 ? ' ticket' : ' tickets')));
    node.appendChild(head);

    const board = el('div', 'board');
    for (const c of cols) board.appendChild(renderColumn(c.col, c.cards));
    node.appendChild(board);
    wrap.appendChild(node);
  }

  if (!wrap.childNodes.length) wrap.appendChild(el('div', 'col-empty', 'no open tickets'));
  return wrap;
}

// ── review queue: what a human owes a decision on ───────────────────────────
//
// Proposals in review (approve from here, or open the page to comment) and the tickets
// sitting in the review column. The approve button posts to the SAME gated verb the CLI
// runs, so a proposal with open threads is refused here exactly as it is there.

function renderReviewQueue(b) {
  const wrap = el('div', 'table-wrap');
  const rows = b.review_queue || [];
  wrap.appendChild(el('h2', 'sec-title', 'Proposals in review (' + rows.length + ')'));
  const t = el('table', 'wt');
  const head = el('tr');
  for (const h of ['proposal', 'title', 'specs', 'threads', 'since', '']) head.appendChild(el('th', null, h));
  t.appendChild(head);
  if (!rows.length) {
    const tr = el('tr');
    const td = el('td', null, 'nothing in review');
    td.colSpan = 6;
    tr.appendChild(td);
    t.appendChild(tr);
  }
  for (const r of rows) {
    const tr = el('tr');
    const id = el('td', 'ref');
    const link = el('a', null, r.id);
    link.href = '/p/' + r.id;
    id.appendChild(link);
    tr.appendChild(id);
    tr.appendChild(el('td', null, r.title));
    tr.appendChild(el('td', 'ref', (r.specs || []).join(', ') || '—'));
    const th = el('td');
    th.appendChild(el('span', 'pill' + (r.unresolved ? ' warn' : ' ok'),
      r.unresolved ? r.unresolved + ' open' : 'nothing open'));
    tr.appendChild(th);
    tr.appendChild(el('td', null, r.created));
    const act = el('td');
    const btn = el('button', 'pill' + (r.unresolved ? ' ghost' : ' go'), 'approve');
    btn.title = r.unresolved ? 'refuses while threads are open' : 'approve and mint the tickets';
    btn.onclick = async () => {
      const res = await fetch('/api/proposal/' + r.id + '/approve', {
        method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}',
      });
      const payload = await res.json().catch(() => null);
      if (!res.ok) {
        const err = (payload && payload.error) || {};
        toast(err.message || ('refused (HTTP ' + res.status + ')'), 'err', err.fix && err.fix[0]);
      } else {
        toast(r.id + ' approved', 'ok');
        refresh();
      }
    };
    act.appendChild(btn);
    tr.appendChild(act);
    t.appendChild(tr);
  }
  wrap.appendChild(t);

  const review = (b.columns || []).find((c) => c.column === 'review');
  const cards = review ? review.cards : [];
  wrap.appendChild(el('h2', 'sec-title', 'Tickets in review (' + cards.length + ')'));
  if (!cards.length) wrap.appendChild(el('p', 'dim', 'nothing shipped for review'));
  else {
    const col = el('div', 'col');
    for (const c of cards) col.appendChild(renderCard(c));
    wrap.appendChild(col);
  }
  return wrap;
}

// ── worktrees: the many-agents-at-a-glance view ─────────────────────────────

function renderWorktrees(rows) {
  const wrap = el('div', 'table-wrap');
  const t = el('table', 'wt');
  const head = el('tr');
  for (const h of ['worktree', 'branch', 'ticket', 'agent', 'last commit', 'ahead / behind main', 'merge state']) {
    head.appendChild(el('th', null, h));
  }
  t.appendChild(head);

  if (!rows.length) {
    const tr = el('tr');
    const td = el('td', null, 'no worktrees');
    td.colSpan = 7;
    tr.appendChild(td);
    t.appendChild(tr);
  }

  for (const r of rows) {
    const tr = el('tr', r.primary ? 'primary' : null);
    const path = el('td', 'path', shortPath(r.path));
    path.title = r.path + (r.primary ? '  (primary worktree)' : '');
    tr.appendChild(path);
    tr.appendChild(el('td', 'ref', r.branch || '—'));

    const tk = el('td', 'ref');
    if (r.ticket) {
      const link = el('span', null, r.ticket);
      link.style.cursor = 'pointer';
      link.onclick = () => openTicket(r.ticket);
      tk.appendChild(link);
    } else {
      tk.textContent = '—';
    }
    tr.appendChild(tk);

    tr.appendChild(el('td', 'ref', r.claimed_by || '—'));
    tr.appendChild(el('td', null, age(r.last_commit_secs)));

    // `git rev-list --left-right --count <main>...<branch>` — DESIGN.md's figure, per row.
    const ab = el('td');
    const box = el('span', 'ab');
    if (r.ahead === null || r.ahead === undefined) {
      box.appendChild(el('span', 'none', 'unknown'));
    } else {
      box.appendChild(el('span', 'a', '↑' + r.ahead));
      box.appendChild(el('span', 'b', '↓' + r.behind));
    }
    ab.appendChild(box);
    tr.appendChild(ab);

    const badge = el('td');
    badge.appendChild(el('span', 'badge ' + r.badge.badge, r.badge_text));
    tr.appendChild(badge);
    t.appendChild(tr);
  }
  wrap.appendChild(t);
  return wrap;
}

// ── rules: exactly what `kanspec rules` prints, from the same endpoint ──────

// Read-only on purpose. Accepting or revoking a decision is a HUMAN act the binary gates
// on `HumanActor` (D-18), and a button in a page anyone on loopback can click is not the
// place to spend that. `kanspec accept D-xxxx` is the fix line, and it is shown as one.
function renderRules() {
  // Refetch in the background when the board moved, and keep showing what we have: a
  // "loading…" flash on every fs event would make the page unreadable while an agent works.
  if (state.rulesStale) {
    state.rulesStale = false;
    fetch('/api/rules')
      .then((r) => r.json())
      .then((d) => { state.rules = d; if (state.tab === 'rules') render(); })
      .catch(() => toast('cannot load the standing rules', 'err', 'kanspec rules'));
  }
  if (!state.rules) return el('div', 'col-empty', 'loading the standing rules…');
  const d = state.rules.data || {};
  const wrap = el('div');

  const section = (title, note) => {
    const s = el('div', 'lane');
    const h = el('div', 'lane-head');
    h.appendChild(el('span', 'nm', title));
    if (note) h.appendChild(el('span', 'feature', note));
    s.appendChild(h);
    wrap.appendChild(s);
    return s;
  };

  const decisions = section('Decisions', 'accepted, in scope first — never edit one, propose a new one');
  for (const x of d.decisions || []) {
    const card = el('div', 'card s-done');
    const top = el('div', 'card-top');
    top.appendChild(el('span', 'id', x.id));
    card.appendChild(top);
    card.appendChild(el('div', 'title', x.title));
    const chips = el('div', 'chips');
    for (const g of x.scope || []) chips.appendChild(el('span', 'chip mono', g));
    chips.appendChild(el('span', 'chip', 'accepted ' + x.accepted));
    card.appendChild(chips);
    if (x.body) {
      const body = el('div');
      body.style.cssText = 'white-space:pre-wrap;color:var(--ink-2);font-size:12px;margin-top:6px';
      body.textContent = x.body;
      card.appendChild(body);
    }
    const foot = el('div', 'card-foot');
    const fix = el('button', 'fix', 'kanspec why ' + x.id);
    fix.onclick = () => copy('kanspec why ' + x.id);
    foot.appendChild(fix);
    card.appendChild(foot);
    decisions.appendChild(card);
  }

  const quirks = section('Quirks', 'active landmines on the paths you are touching');
  for (const q of d.quirks || []) {
    const card = el('div', 'card s-doing');
    card.appendChild(el('div', 'title', q.title));
    const chips = el('div', 'chips');
    chips.appendChild(el('span', 'chip disc', q.severity));
    for (const g of q.paths || []) chips.appendChild(el('span', 'chip mono', g));
    card.appendChild(chips);
    quirks.appendChild(card);
  }

  const rules = section('Spec rules', 'with their proposal provenance');
  for (const r of d.spec_rules || []) {
    const line = el('div', 'att');
    line.appendChild(el('span', 'subj', r.spec + '#' + r.anchor));
    line.appendChild(el('span', 'txt', r.text));
    for (const p of r.provenance || []) line.appendChild(el('span', 'chip proposal', p));
    rules.appendChild(line);
  }

  const note = el('div', 'col-empty');
  note.textContent = 'This page is exactly what `kanspec rules` prints — the one standing set agents are steered by.';
  wrap.appendChild(note);
  return wrap;
}

// ── drag targets → verbs ────────────────────────────────────────────────────

const VERB_FOR_COLUMN = {
  ready: 'park',
  backlog: 'park',
  doing: 'start',
  review: 'ship',
  done: 'done',
};

function dropOnto(id, column) {
  const name = VERB_FOR_COLUMN[column];
  if (!name) {
    toast('IN MAIN is derived from git — nothing can be dragged into it', 'err', 'kanspec scan');
    return;
  }
  if (name === 'park') {
    const why = prompt('park ' + id + ' — why are you putting it down?');
    if (!why) return;
    verb(id, 'park', { why });
    return;
  }
  verb(id, name, {});
}

// ── the drawer ──────────────────────────────────────────────────────────────

async function openTicket(id) {
  state.open = id;
  history.replaceState(null, '', '/t/' + id);
  const d = $('drawer');
  clear(d);
  d.hidden = false;
  $('scrim').hidden = false;
  d.appendChild(el('div', 'sub', 'loading ' + id + '…'));

  let t = null;
  try {
    const res = await fetch('/api/ticket/' + encodeURIComponent(id));
    const payload = await res.json();
    if (!res.ok) {
      const err = payload.error || {};
      clear(d);
      d.appendChild(closeButton());
      d.appendChild(el('h2', null, err.message || 'not found'));
      if (err.fix && err.fix.length) d.appendChild(el('code', 'fixline', err.fix[0]));
      return;
    }
    t = payload;
  } catch (e) {
    clear(d);
    d.appendChild(closeButton());
    d.appendChild(el('h2', null, 'cannot reach the server'));
    return;
  }

  clear(d);
  d.appendChild(closeButton());
  d.appendChild(el('div', 'sub', (GLYPH[t.state] || '?') + ' ' + t.id + ' · ' + t.state));
  d.appendChild(el('h2', null, t.title));

  const badge = el('div');
  badge.style.marginTop = '8px';
  badge.appendChild(el('span', 'badge ' + t.badge.badge, t.badge_text));
  d.appendChild(badge);

  const facts = el('section');
  facts.appendChild(el('h3', null, 'FACTS'));
  const kv = el('dl', 'kv');
  const rows = [
    ['column', t.column],
    ['spec', t.spec],
    ['proposal', t.proposal],
    ['branch', t.branch],
    ['worktree', t.worktree],
    ['claimed by', t.claimed_by],
    ['pr', t.pr ? '#' + t.pr : null],
    ['head', t.head],
    ['blocked by', (t.blocked_by || []).join(', ')],
  ];
  for (const [k, v] of rows) {
    if (!v) continue;
    kv.appendChild(el('dt', null, k));
    kv.appendChild(el('dd', k === 'branch' || k === 'worktree' || k === 'head' ? 'mono' : null, v));
  }
  facts.appendChild(kv);
  d.appendChild(facts);

  if (t.steps && t.steps.length) {
    const s = el('section');
    s.appendChild(el('h3', null, 'STEPS'));
    const ul = el('ul', 'steps');
    for (const step of t.steps) {
      ul.appendChild(el('li', step.done ? 'done' : null, (step.done ? '[x] ' : '[ ] ') + step.text));
    }
    s.appendChild(ul);
    d.appendChild(s);
  }

  d.appendChild(verbsFor(t));

  if (t.log && t.log.length) {
    const s = el('section');
    s.appendChild(el('h3', null, 'LOG'));
    const box = el('div', 'log');
    for (const e of t.log) {
      box.appendChild(el('div', null, e.at + '  ' + e.state + '  ' + e.actor + '  ' + e.verb + (e.note ? ' (' + e.note + ')' : '')));
    }
    s.appendChild(box);
    d.appendChild(s);
  }
}

// The board IS the state machine — but the legality oracle lives in the binary, so these
// buttons are a convenience, not a second copy of the transition table. Every one of them
// can still be refused, and the refusal is what gets shown.
function verbsFor(t) {
  const s = el('section');
  s.appendChild(el('h3', null, 'VERBS'));
  const box = el('div', 'verbs');
  const add = (label, name, danger, ask) => {
    const b = el('button', 'verb' + (danger ? ' danger' : ''), label);
    b.onclick = () => {
      let body = {};
      if (ask) {
        const why = prompt(name + ' ' + t.id + ' — why?');
        if (!why) return;
        body = { why };
      }
      verb(t.id, name, body);
    };
    box.appendChild(b);
  };
  add('start', 'start', false, false);
  add('ship', 'ship', false, false);
  add('done', 'done', false, false);
  add('park', 'park', false, true);
  add('drop', 'drop', true, true);
  s.appendChild(box);

  const hint = el('div', 'sub');
  hint.style.marginTop = '8px';
  hint.textContent = 'done runs the full gate — it refuses without a git-detected merge, and prints why.';
  s.appendChild(hint);
  return s;
}

function closeButton() {
  const b = el('button', 'close', '✕');
  b.onclick = closeDrawer;
  return b;
}

function closeDrawer() {
  state.open = null;
  $('drawer').hidden = true;
  $('scrim').hidden = true;
  history.replaceState(null, '', '/');
}

// ── wiring ──────────────────────────────────────────────────────────────────

function applyTheme(t) {
  if (t) document.documentElement.setAttribute('data-theme', t);
  else document.documentElement.removeAttribute('data-theme');
}

// ── the review page ─────────────────────────────────────────────────────────
//
// `/p/<id>`. The proposal typeset properly, with each `[cN]`/`[pN]`/`[tN]` clickable and
// its threads in a right-hand rail. Every write goes through the SAME `cmd::*` handler the
// CLI calls, so a comment typed here and one typed in a terminal are the same bytes on
// disk — the page is a view of the repo, never a second store.

async function loadReview(id) {
  try {
    const res = await fetch('/api/proposal/' + encodeURIComponent(id), {
      headers: { accept: 'application/json' },
    });
    const payload = await res.json().catch(() => null);
    if (!res.ok) {
      const err = (payload && payload.error) || {};
      renderReviewError(id, err.message || ('HTTP ' + res.status), err.fix && err.fix[0]);
      return;
    }
    state.review = payload;
    renderReview();
  } catch (e) {
    renderReviewError(id, 'cannot reach the server: ' + e.message);
  }
}

function renderReviewError(id, message, fix) {
  const main = $('main');
  clear(main);
  const box = el('div', 'review-error');
  box.appendChild(el('h1', null, id));
  box.appendChild(el('p', null, message));
  if (fix) box.appendChild(el('code', null, fix));
  const back = el('a', 'pill ghost', '← the board');
  back.href = '/';
  box.appendChild(back);
  main.appendChild(box);
}

// A review POST, with the CLI's own refusal envelope surfaced as a toast.
async function reviewPost(path, body) {
  try {
    const res = await fetch(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body || {}),
    });
    const payload = await res.json().catch(() => null);
    if (!res.ok) {
      const err = (payload && payload.error) || {};
      toast(err.message || ('refused (HTTP ' + res.status + ')'), 'err', err.fix && err.fix[0]);
      return false;
    }
    await loadReview(state.review.id);
    return true;
  } catch (e) {
    toast('the request did not reach the server: ' + e.message, 'err');
    return false;
  }
}

function renderReview() {
  const p = state.review;
  if (!p) return;
  document.title = p.id + ' · ' + p.title;
  $('attention').hidden = true;
  $('features').hidden = true;
  for (const t of document.querySelectorAll('.tab')) t.setAttribute('aria-selected', 'false');

  const main = $('main');
  clear(main);
  const wrap = el('div', 'review');

  // ── the proposal, typeset ──
  const doc = el('article', 'review-doc');
  const head = el('header', 'review-head');
  const crumb = el('a', 'crumb', '← the board');
  crumb.href = '/';
  head.appendChild(crumb);
  head.appendChild(el('h1', null, p.title));

  const meta = el('div', 'review-meta');
  meta.appendChild(el('span', 'pill mono', p.id));
  meta.appendChild(el('span', 'pill status-' + p.status, p.status));
  for (const sp of p.specs) meta.appendChild(el('span', 'pill ghost', 'spec ' + sp));
  if (p.approved) meta.appendChild(el('span', 'pill ok', 'approved ' + p.approved));
  head.appendChild(meta);
  doc.appendChild(head);

  if (p.why) {
    const why = el('section', 'review-sec');
    why.appendChild(el('h2', null, 'Why'));
    why.appendChild(renderFold(p.why_headline || p.why, p.why_detail || '', 'prose'));
    doc.appendChild(why);
  }

  // Skimmable by construction: every section shows one headline per bullet and folds the
  // rest. Reviewers read the plan in a dozen lines and open only what they doubt.
  const groups = [
    ['c', 'Changes', null],
    ['p', 'Rules this leaves behind',
      'kanspec calls these prescriptions. A closed proposal binds nothing, so a rule that ' +
      'should keep steering agents afterwards is named here and typed: promote → decision ' +
      '(a human accepts it), promote → spec (it becomes a rule bullet), or temp until a ' +
      'ticket lands (then it expires).'],
    ['t', 'Tickets', null],
  ];
  const sections = p.sections || [];
  for (const [kind, label, note] of groups) {
    const items = p.items.filter((i) => i.kind === kind);
    if (!items.length) continue;
    const sec = el('section', 'review-sec');
    sec.appendChild(el('h2', null, label));
    if (note) sec.appendChild(el('p', 'sec-note', note));
    for (const item of items) sec.appendChild(renderItem(item));
    // DESIGN: the current spec text inlined under Changes, collapsible — so the delta is
    // reviewed against today's truth without opening a file.
    if (kind === 'c' && p.context.length) sec.appendChild(renderContext(p.context));
    doc.appendChild(sec);
    // The author's own sections (Testing and verification, Security impact, …) sit
    // between Changes and the rules, in file order — never dropped.
    if (kind === 'c') for (const s of sections) doc.appendChild(renderSection(s));
  }
  wrap.appendChild(doc);

  // ── the rail ──
  wrap.appendChild(renderRail(p));
  main.appendChild(wrap);
}

function anchorOf(item) {
  return item.id.proposal + '#' + item.kind + item.id.n;
}

// A headline with its detail folded under it. Click the headline or the chevron to open;
// nothing is hidden, it is one tap away.
function renderFold(headline, detail, cls) {
  const box = el('div', 'fold' + (cls ? ' ' + cls : ''));
  const head = el('span', 'fold-head', headline);
  box.appendChild(head);
  if (detail) {
    const more = el('button', 'fold-more', '›');
    more.type = 'button';
    more.setAttribute('aria-expanded', 'false');
    more.title = 'show the detail';
    const body = el('div', 'fold-detail', detail);
    body.hidden = true;
    const toggle = () => {
      body.hidden = !body.hidden;
      more.setAttribute('aria-expanded', String(!body.hidden));
      box.classList.toggle('open', !body.hidden);
    };
    more.onclick = toggle;
    head.onclick = toggle;
    head.classList.add('has-more');
    box.appendChild(more);
    box.appendChild(body);
  }
  return box;
}

function renderSection(s) {
  const sec = el('section', 'review-sec');
  sec.appendChild(el('h2', null, s.heading));
  if (s.prose) sec.appendChild(el('p', 'prose', s.prose));
  for (const b of s.bullets) {
    const row = el('div', 'item plain');
    row.appendChild(el('span', 'item-dot', '·'));
    const body = el('div', 'item-body');
    body.appendChild(renderFold(b.headline, b.detail));
    row.appendChild(body);
    sec.appendChild(row);
  }
  return sec;
}

function renderItem(item) {
  const row = el('div', 'item' + (item.dispositioned ? ' done' : ''));
  row.id = 'item-' + item.kind + item.id.n;
  const tag = el('button', 'item-tag', '[' + item.kind + item.id.n + ']');
  tag.title = 'comment on this item';
  tag.onclick = () => startThread(anchorOf(item));
  row.appendChild(tag);

  const body = el('div', 'item-body');
  body.appendChild(renderFold(item.headline || item.text, item.detail || '', 'item-text'));
  if (item.badge) {
    const cls = item.badge.startsWith('TEMP')
      ? 'badge temp'
      : item.badge.startsWith('UNTYPED')
        ? 'badge untyped'
        : 'badge promote';
    body.appendChild(el('span', cls, item.badge));
  }
  if (item.ticket) {
    const link = el('a', 'badge ticket', item.ticket);
    link.href = '/t/' + item.ticket;
    body.appendChild(link);
  }
  const open = item.threads.filter((t) => !t.resolved).length;
  if (item.threads.length) {
    const chip = el('button', 'badge threads' + (open ? ' open' : ''),
      item.threads.length + (open ? ' · ' + open + ' open' : ' resolved'));
    chip.onclick = () => {
      const first = document.getElementById('thread-' + item.threads[0].id);
      if (first) first.scrollIntoView({ behavior: 'smooth', block: 'center' });
    };
    body.appendChild(chip);
  }
  row.appendChild(body);
  return row;
}

function renderContext(context) {
  const box = el('details', 'context');
  const sum = el('summary', null, 'the specs as they stand today');
  box.appendChild(sum);
  for (const c of context) {
    const b = el('div', 'context-spec');
    b.appendChild(el('h3', null, c.spec + ' — ' + c.feature));
    if (!c.rules.length) {
      b.appendChild(el('p', 'dim', 'no rules yet'));
    }
    for (const r of c.rules) {
      const line = el('div', 'context-rule');
      line.appendChild(el('span', 'mono dim', '[' + r.anchor + ']'));
      line.appendChild(el('span', null, ' ' + r.text));
      b.appendChild(line);
    }
    box.appendChild(b);
  }
  return box;
}

function renderRail(p) {
  const rail = el('aside', 'rail');
  const head = el('div', 'rail-head');
  head.appendChild(el('h2', null, 'Review'));
  const n = p.unresolved;
  head.appendChild(el('span', 'pill' + (n ? ' warn' : ' ok'),
    n ? n + ' open' : 'nothing open'));
  rail.appendChild(head);

  // The gate, stated where the button is, so a refusal is never a surprise.
  const act = el('div', 'rail-act');
  if (p.status === 'draft') {
    const b = el('button', 'pill', 'put up for review');
    b.onclick = () => reviewPost('/api/proposal/' + p.id + '/review');
    act.appendChild(b);
  } else if (p.status === 'review') {
    const b = el('button', 'pill' + (n ? ' ghost' : ' go'), 'approve');
    b.disabled = false;
    b.title = p.blocked_by ? 'refuses while ' + p.blocked_by : 'approve and mint the tickets';
    b.onclick = () => reviewPost('/api/proposal/' + p.id + '/approve');
    act.appendChild(b);
    if (p.blocked_by) act.appendChild(el('span', 'dim', 'blocked by ' + p.blocked_by));
  } else {
    act.appendChild(el('span', 'dim', p.status));
  }
  rail.appendChild(act);

  const threads = [];
  for (const item of p.items) for (const t of item.threads) threads.push([item, t]);
  if (!threads.length && !p.orphaned.length) {
    rail.appendChild(el('p', 'dim', 'Click any [c1] to start a thread.'));
  }
  for (const [item, t] of threads) rail.appendChild(renderThread(t, anchorOf(item)));

  if (p.orphaned.length) {
    // Invariant 5: a deleted item does not delete the objection to it.
    rail.appendChild(el('h3', 'orphan-head', 'Orphaned (' + p.orphaned.length + ')'));
    rail.appendChild(el('p', 'dim', 'the item these point at is gone'));
    for (const t of p.orphaned) rail.appendChild(renderThread(t, t.target, true));
  }
  return rail;
}

// `trevor` / `trevor via agent`: the row's label, and the kind when an agent wrote it.
function who(label, via) {
  const name = label || 'someone';
  return via ? name + ' via ' + via : name;
}

function renderThread(t, target, orphan) {
  const box = el('div', 'thread' + (t.resolved ? ' resolved' : '') + (orphan ? ' orphan' : ''));
  box.id = 'thread-' + t.id;
  const head = el('div', 'thread-head');
  head.appendChild(el('span', 'mono', target));
  if (t.edited_since) head.appendChild(el('span', 'badge edited', 'edited since'));
  box.appendChild(head);

  if (t.quote) box.appendChild(el('blockquote', null, t.quote));
  const first = el('div', 'msg');
  first.appendChild(el('span', 'who', who(t.author, t.via)));
  first.appendChild(el('span', null, t.body));
  box.appendChild(first);

  for (const r of t.replies) {
    const m = el('div', 'msg reply');
    m.appendChild(el('span', 'who', who(r.by, r.via)));
    m.appendChild(el('span', null, r.body));
    box.appendChild(m);
  }

  if (t.resolved !== null && t.resolved !== undefined) {
    box.appendChild(el('div', 'resolution', '✓ ' + (t.resolved || 'resolved')));
    return box;
  }

  const acts = el('div', 'thread-acts');
  const reply = el('button', 'link', 'reply');
  reply.onclick = () => promptThen('Reply', (v) =>
    reviewPost('/api/comment/' + t.id + '/reply', { body: v }));
  acts.appendChild(reply);
  const res = el('button', 'link', 'resolve');
  // The note is required by the CLI, and it is required here for the same reason: what
  // changed IS the resolution.
  res.onclick = () => promptThen('What changed? (required)', (v) =>
    reviewPost('/api/comment/' + t.id + '/resolve', { note: v }));
  acts.appendChild(res);
  box.appendChild(acts);
  return box;
}

function startThread(target) {
  promptThen('Comment on ' + target, (v) =>
    reviewPost('/api/proposal/' + state.review.id + '/comment', { target, body: v }));
}

// A tiny inline composer rather than `prompt()`: a modal dialog blocks the SSE refresh and
// cannot be styled to match, and this page is where a human does their reading.
function promptThen(label, run) {
  const old = document.querySelector('.composer');
  if (old) old.remove();
  const box = el('div', 'composer');
  box.appendChild(el('label', null, label));
  const ta = document.createElement('textarea');
  ta.rows = 3;
  box.appendChild(ta);
  const acts = el('div', 'composer-acts');
  const ok = el('button', 'pill go', 'send');
  const no = el('button', 'pill ghost', 'cancel');
  ok.onclick = async () => {
    const v = ta.value.trim();
    if (!v) { ta.focus(); return; }
    box.remove();
    await run(v);
  };
  no.onclick = () => box.remove();
  acts.appendChild(ok);
  acts.appendChild(no);
  box.appendChild(acts);
  document.body.appendChild(box);
  ta.focus();
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) ok.click();
    if (e.key === 'Escape') no.click();
  });
}

function boot() {
  applyTheme(localStorage.getItem('ks.theme'));

  $('tabs').addEventListener('click', (e) => {
    const tab = e.target.closest('.tab');
    if (!tab) return;
    state.tab = tab.dataset.tab;
    localStorage.setItem('ks.tab', state.tab);
    render();
  });

  $('theme').onclick = () => {
    // With nothing chosen yet the page is following the system, so the first click has to
    // flip away from what the system is actually showing — not blindly to dark.
    const chosen = document.documentElement.getAttribute('data-theme');
    const showing = chosen
      || (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
    const next = showing === 'dark' ? 'light' : 'dark';
    localStorage.setItem('ks.theme', next);
    applyTheme(next);
  };

  $('rescan').onclick = async () => {
    $('rescan').textContent = 'scanning…';
    try {
      const res = await fetch('/api/scan', { method: 'POST' });
      const p = await res.json().catch(() => null);
      if (!res.ok) {
        const err = (p && p.error) || {};
        toast(err.message || 'scan failed', 'err', err.fix && err.fix[0]);
      } else {
        toast(p.scanned + ' scanned · ' + p.landed.length + ' in main · ' + p.unknown.length + ' unknown', 'ok');
      }
    } catch (e) {
      toast('scan did not reach the server', 'err');
    }
    $('rescan').textContent = 'scan';
    refresh();
  };

  $('scrim').onclick = closeDrawer;
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeDrawer(); });
  window.addEventListener('focus', refresh);

  // `/p/<id>` is its own page, not a drawer over the board: reviewing is reading, and the
  // board's columns behind it would be noise.
  const rev = location.pathname.match(/^\/p\/([^/]+)$/);
  if (rev) {
    document.body.classList.add('reviewing');
    loadReview(decodeURIComponent(rev[1]));
    subscribe();
    return;
  }

  const deep = location.pathname.match(/^\/t\/([^/]+)$/);
  refresh().then(() => { if (deep) openTicket(decodeURIComponent(deep[1])); });
  subscribe();
}

boot();
