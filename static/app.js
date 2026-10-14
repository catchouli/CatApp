// Front end: a small hash-routed single-page app over the JSON API. The app's name comes from
// the server (config.json), so the same code serves CatApp, MaisyApp, ...
'use strict';

const app = document.getElementById('app');
let meta = { statuses: [], categories: [] };
// Statuses meaning no more work is planned: hidden unless "Show finished" is on.
const FINISHED = ['done', 'abandoned', 'archived'];
let pref = loadPref();

function loadPref() {
  try { return JSON.parse(localStorage.getItem('catapp') || '{}'); } catch { return {}; }
}
function savePref() {
  try { localStorage.setItem('catapp', JSON.stringify(pref)); } catch { /* private mode */ }
}

// ---- helpers ----------------------------------------------------------------------

async function api(method, path, body) {
  const opts = { method, headers: {} };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  const r = await fetch(path, opts);
  if (r.status === 401) { location.reload(); throw new Error('Not logged in'); }
  let data = null;
  try { data = await r.json(); } catch { /* empty */ }
  if (!r.ok) throw new Error((data && data.error) || r.statusText);
  return data;
}

function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// Escaped text with links and `code` made live. Newlines are kept by CSS.
function rich(s) {
  return esc(s)
    .replace(/`([^`\n]+)`/g, '<code>$1</code>')
    .replace(/\bhttps?:\/\/[^\s<>"']+[^\s<>"'.,;:!?)\]]/g, u => `<a href="${u}" target="_blank" rel="noopener">${u}</a>`);
}

function ago(ts) {
  const t = new Date(ts);
  if (isNaN(t)) return '';
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 172800) return 'yesterday';
  if (s < 604800) return `${Math.floor(s / 86400)}d ago`;
  return t.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: t.getFullYear() === new Date().getFullYear() ? undefined : 'numeric' });
}

function fullTime(ts) {
  const t = new Date(ts);
  return isNaN(t) ? '' : t.toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
}

// Notes written by people come from the web app or the CLI; anything else is an agent.
const isAgent = n => !['web', 'cli'].includes(n.source);

function cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }
const pill = s => `<span class="pill s-${esc(s)}">${esc(s)}</span>`;
const srcBadge = s => (s && s !== 'web') ? `<span class="badge" title="Added from ${esc(s)}">${esc(s)}</span>` : '';

let toastTimer;
function toast(msg, err) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = 'show' + (err ? ' err' : '');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.className = ''; }, err ? 4000 : 1800);
}

async function guard(fn) {
  try { return await fn(); } catch (e) { toast(e.message || String(e), true); }
}

function catOrder(cats) {
  const order = ['personal', 'work'];
  return [...cats].sort((a, b) => {
    const ia = order.indexOf(a), ib = order.indexOf(b);
    return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
  });
}

function statusRank(s) {
  const i = meta.statuses.indexOf(s);
  return i < 0 ? 99 : i;
}

// Submit a textarea with Cmd/Ctrl+Enter.
function submitOnCmdEnter(textarea, fn) {
  textarea.addEventListener('keydown', e => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); fn(); }
  });
}

// ---- routing ----------------------------------------------------------------------

async function render() {
  const h = location.hash.replace(/^#/, '') || '/';
  try {
    if (!meta.statuses.length) meta = await api('GET', '/api/meta');
    if (h.startsWith('/p/')) await renderProject(decodeURIComponent(h.slice(3)));
    else if (h === '/new') renderNew();
    else if (h.startsWith('/search')) await renderSearch(decodeURIComponent(h.split('q=')[1] || ''));
    else if (h === '/ask') { history.replaceState(null, '', '#/'); await renderHome(); openAsk(); }
    else await renderHome();
  } catch (e) {
    app.innerHTML = `<p class="loading">${esc(e.message)}</p><p style="text-align:center"><a href="#/">Back to all projects</a></p>`;
  }
}

window.addEventListener('hashchange', () => { window.scrollTo(0, 0); render(); });

// Refresh when coming back to the tab (notes may have come from the CLI or
// Claude), unless something is being typed.
function busyTyping() {
  const a = document.activeElement;
  if (a && (a.tagName === 'TEXTAREA' || a.tagName === 'INPUT') && a.value) return true;
  return [...document.querySelectorAll('textarea')].some(t => t.value.trim());
}
document.addEventListener('visibilitychange', () => { if (!document.hidden && !busyTyping()) render(); });
setInterval(() => { if (!document.hidden && !busyTyping()) render(); }, 60000);

// ---- home -------------------------------------------------------------------------

async function renderHome() {
  const [allProjects, recent] = await Promise.all([api('GET', '/api/projects'), api('GET', '/api/recent?limit=12')]);
  // The project with slug "todo" is the general to-do list: shown as a panel, not a card.
  const todo = allProjects.some(p => p.slug === TODO_SLUG) ? await api('GET', `/api/projects/${TODO_SLUG}`) : null;
  const projects = allProjects.filter(p => p.slug !== TODO_SLUG);
  const filter = pref.filter || 'all';
  const showFinished = !!pref.showFinished;
  const cats = catOrder([...new Set(projects.map(p => p.category))]);

  const visible = projects
    .filter(p => filter === 'all' || p.category === filter)
    .filter(p => showFinished || !FINISHED.includes(p.status));
  const finishedCount = projects.filter(p => (filter === 'all' || p.category === filter) && FINISHED.includes(p.status)).length;

  const groups = catOrder([...new Set(visible.map(p => p.category))]).map(cat => {
    const ps = visible.filter(p => p.category === cat)
      .sort((a, b) => statusRank(a.status) - statusRank(b.status) || b.last_activity.localeCompare(a.last_activity));
    return `<section class="group"><h2 class="section-title">${esc(cap(cat))}</h2><div class="cards">${ps.map(card).join('')}</div></section>`;
  }).join('');

  const active = projects.filter(p => !FINISHED.includes(p.status))
    .sort((a, b) => b.last_activity.localeCompare(a.last_activity));

  app.innerHTML = `
    <div class="home">
      <div>
        <div class="chips" role="group" aria-label="Filter by category">
          ${['all', ...cats].map(c => `<button class="chip" data-filter="${esc(c)}" aria-pressed="${c === filter}">${esc(cap(c))}</button>`).join('')}
          <button class="chip" data-finished aria-pressed="${showFinished}">${showFinished ? 'Hide' : 'Show'} finished${finishedCount ? ` (${finishedCount})` : ''}</button>
        </div>
        ${groups || `<div class="panel"><p class="empty">No projects here yet.</p><a class="btn primary" href="#/new">New project</a></div>`}
      </div>
      <aside>
        ${todoPanel(todo)}
        ${active.length ? `
        <div class="panel quick">
          <h2>Quick note</h2>
          <textarea id="qn" placeholder="What happened, what you decided, what's next…" aria-label="Note"></textarea>
          <div class="row">
            <select id="qp" aria-label="Project">${active.map(p => `<option value="${esc(p.slug)}" ${p.slug === pref.lastProject ? 'selected' : ''}>${esc(p.name)}</option>`).join('')}</select>
            <button class="primary" id="qs">Add</button>
          </div>
        </div>` : ''}
        <div class="panel">
          <h2>Recent notes</h2>
          ${recent.length ? `<ul class="feed">${recent.map(n => `
            <li>
              <div class="who"><a href="#/p/${encodeURIComponent(n.project_slug)}">${esc(n.project_name)}</a> · <span title="${esc(fullTime(n.created_at))}">${ago(n.created_at)}</span> ${srcBadge(n.source)}</div>
              <div class="body">${rich(n.body)}</div>
            </li>`).join('')}</ul>` : '<p class="empty">Notes you add will show up here.</p>'}
        </div>
      </aside>
    </div>`;

  app.querySelectorAll('[data-filter]').forEach(b => b.onclick = () => { pref.filter = b.dataset.filter; savePref(); renderHome(); });
  app.querySelector('[data-finished]').onclick = () => { pref.showFinished = !showFinished; savePref(); renderHome(); };

  const qs = document.getElementById('qs');
  if (qs) {
    const qn = document.getElementById('qn'), qp = document.getElementById('qp');
    const send = () => guard(async () => {
      if (!qn.value.trim()) return qn.focus();
      qs.disabled = true;
      try {
        await api('POST', `/api/projects/${encodeURIComponent(qp.value)}/notes`, { body: qn.value });
        pref.lastProject = qp.value; savePref();
        qn.value = '';
        toast('Note added');
        await renderHome();
      } finally { qs.disabled = false; }
    });
    qs.onclick = send;
    submitOnCmdEnter(qn, send);
  }
  wireTodo(todo);
}

// ---- to-do list -------------------------------------------------------------------

const TODO_SLUG = 'todo';
const TODO_SHOWN = 8;

function todoPanel(todo) {
  if (!todo) return '';
  const allOpen = (pref.newestTasksFirst ? [...todo.tasks].reverse() : todo.tasks).filter(t => !t.done);
  const done = todo.tasks.length - allOpen.length;
  const open = allOpen.slice(0, TODO_SHOWN);
  const more = allOpen.length - open.length;
  return `
    <div class="panel todo">
      <h2>To do <span>${allOpen.length > 1 ? `<button class="ghost" data-todoorder aria-pressed="${!!pref.newestTasksFirst}" title="Change to-do order">${pref.newestTasksFirst ? 'Newest first' : 'Oldest first'}</button> ` : ''}<a class="hint" href="#/p/${TODO_SLUG}">${done ? `${done} done · ` : ''}all</a></span></h2>
      ${open.length ? `<ul class="tasks">${open.map(t => `
        <li>
          <input type="checkbox" data-todo="${t.id}" aria-label="${esc(t.text)}">
          <span class="t" title="Tap to expand">${rich(t.text)}</span>
        </li>`).join('')}</ul>${more ? `<p class="hint" style="margin:0 0 10px"><a href="#/p/${TODO_SLUG}">+ ${more} more</a></p>` : ''}` : '<p class="empty">Nothing to do. Nice.</p>'}
      <form id="todo-form" class="row"><input id="todo-in" placeholder="Add a to-do" aria-label="New to-do" style="flex:1;min-width:0"><button>Add</button></form>
    </div>`;
}

function wireTodo(todo) {
  if (!todo) return;
  const order = app.querySelector('[data-todoorder]');
  if (order) order.onclick = () => { pref.newestTasksFirst = !pref.newestTasksFirst; savePref(); renderHome(); };
  app.querySelectorAll('.todo .t').forEach(t => t.onclick = e => {
    if (e.target.tagName !== 'A') t.classList.toggle('open');
  });
  app.querySelectorAll('[data-todo]').forEach(cb => cb.onchange = () => guard(async () => {
    await api('PATCH', `/api/tasks/${cb.dataset.todo}`, { done: cb.checked });
    toast('Done ✓');
    renderHome();
  }));
  document.getElementById('todo-form').onsubmit = e => {
    e.preventDefault();
    const inp = document.getElementById('todo-in');
    if (!inp.value.trim()) return inp.focus();
    guard(async () => {
      await api('POST', `/api/projects/${TODO_SLUG}/tasks`, { text: inp.value });
      await renderHome();
      document.getElementById('todo-in').focus();
    });
  };
}

function card(p) {
  const pct = p.tasks_total ? Math.round(100 * p.tasks_done / p.tasks_total) : 0;
  return `
    <a class="card" href="#/p/${encodeURIComponent(p.slug)}">
      <div class="card-top"><span class="name">${esc(p.name)}</span>${pill(p.status)}</div>
      ${p.next_step ? `<div class="next">Next: ${esc(p.next_step)}</div>` : ''}
      <div class="meta">
        ${p.tasks_total ? `<span class="bar" aria-hidden="true"><i style="width:${pct}%"></i></span><span>${p.tasks_done}/${p.tasks_total} tasks</span>` : ''}
        ${p.note_count ? `<span>${p.note_count} note${p.note_count === 1 ? '' : 's'}</span>` : ''}
        <span title="${esc(fullTime(p.last_activity))}">${ago(p.last_activity)}</span>
      </div>
    </a>`;
}

// ---- project page -----------------------------------------------------------------

async function renderProject(slug) {
  const p = await api('GET', `/api/projects/${encodeURIComponent(slug)}`);
  const base = `/api/projects/${encodeURIComponent(p.slug)}`;
  const linkLines = p.links.split('\n').map(l => l.trim()).filter(Boolean);
  const logFilter = pref.logFilter || 'all';
  const showDone = !!pref.showDoneTasks;
  const newestFirst = !!pref.newestTasksFirst;
  const pick = { all: () => true, mine: n => !isAgent(n) && n.kind === 'note', agents: isAgent };
  const counts = { mine: p.notes.filter(pick.mine).length, agents: p.notes.filter(pick.agents).length };
  const notes = p.notes.filter(pick[logFilter] || pick.all);

  app.innerHTML = `
    <a class="back" href="#/">← All projects</a>
    <div class="proj-head">
      <div>
        <h1>${esc(p.name)}</h1>
        <div class="sub">${esc(cap(p.category))} · started ${esc(new Date(p.created_at).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' }))}</div>
      </div>
      <label style="margin:0">
        <span class="hint" style="display:block;margin-bottom:4px">Status</span>
        <select id="status">${meta.statuses.map(s => `<option ${s === p.status ? 'selected' : ''}>${s}</option>`).join('')}</select>
      </label>
    </div>

    <div class="project">
      <section class="panel a-next">
        <h2>Next step <button class="ghost" data-edit="next">Edit</button></h2>
        <div id="next-view">${p.next_step ? `<div class="nextstep">${rich(p.next_step)}</div>` : '<p class="empty">No next step set. What\'s the very next thing to do?</p>'}</div>
      </section>

      <section class="panel a-notes">
        <h2>Log <span class="chips" style="margin:0" role="group" aria-label="Show">${['all', 'mine', 'agents'].map(f => `<button class="chip small" data-logf="${f}" aria-pressed="${f === logFilter}">${cap(f)}${counts[f] ? ` ${counts[f]}` : ''}</button>`).join('')}</span></h2>
        <textarea id="note" placeholder="Add a note: progress, decisions, ideas, blockers…" aria-label="New note"></textarea>
        <div class="row end" style="margin-top:8px"><span class="hint" style="margin-right:auto">⌘/Ctrl + Enter</span><button class="primary" id="add-note">Add note</button></div>
        <ul class="log">${notes.map(n => `
          <li class="${n.kind === 'note' ? '' : 'sys'}${isAgent(n) ? ' agent' : ''}" data-note="${n.id}">
            <div class="when">
              <span title="${esc(fullTime(n.created_at))}">${esc(fullTime(n.created_at))}</span>${srcBadge(n.source)}${n.edited_at ? '<span class="hint">edited</span>' : ''}
              <span class="actions">${n.kind === 'note' ? '<button class="ghost" data-act="edit">Edit</button>' : ''}<button class="ghost" data-act="del">Delete</button></span>
            </div>
            <div class="body">${rich(n.body)}</div>
          </li>`).join('') || ''}</ul>
      </section>

      <section class="panel a-tasks">
        <h2><span>Tasks ${p.tasks_total ? `<span class="hint">${p.tasks_done}/${p.tasks_total}</span>` : ''}</span><span>${p.tasks_total > 1 ? `<button class="ghost" data-taskorder aria-pressed="${newestFirst}" title="Change task order">${newestFirst ? 'Newest first' : 'Oldest first'}</button>` : ''}${p.tasks_done ? ` <button class="ghost" data-showdone aria-pressed="${showDone}">${showDone ? 'Hide done' : `Show done (${p.tasks_done})`}</button>` : ''}</span></h2>
        ${!showDone && p.tasks_total && p.tasks_total === p.tasks_done ? `<p class="empty">All done.</p>` : ''}
        <ul class="tasks">${(newestFirst ? [...p.tasks].reverse() : p.tasks).filter(t => showDone || !t.done).map(t => `
          <li class="${t.done ? 'done' : ''}">
            <input type="checkbox" data-task="${t.id}" ${t.done ? 'checked' : ''} aria-label="${esc(t.text)}">
            <span class="t">${rich(t.text)}</span>
            <button class="ghost x" data-deltask="${t.id}" aria-label="Delete task">✕</button>
          </li>`).join('')}</ul>
        <form id="task-form" class="row"><input id="task" placeholder="Add a task" aria-label="New task" style="flex:1;min-width:0"><button>Add</button></form>
      </section>

      <section class="panel a-details">
        <h2>Details <button class="ghost" data-edit="details">Edit</button></h2>
        <div id="details-view">
          ${p.summary ? `<div class="prose">${rich(p.summary)}</div>` : '<p class="empty">No summary yet.</p>'}
          ${linkLines.length ? `<div class="links" style="margin-top:10px">${linkLines.map(l => /^https?:\/\//.test(l) ? `<a href="${esc(l)}" target="_blank" rel="noopener">${esc(l)}</a>` : `<span>${rich(l)}</span>`).join('')}</div>` : ''}
        </div>
      </section>
    </div>`;

  // Status
  document.getElementById('status').onchange = e => guard(async () => {
    await api('PATCH', base, { status: e.target.value });
    toast(`Status: ${e.target.value}`);
    renderProject(p.slug);
  });

  // Next step editor
  app.querySelector('[data-edit="next"]').onclick = () => {
    const v = document.getElementById('next-view');
    v.innerHTML = `<textarea id="next-in" style="min-height:64px">${esc(p.next_step)}</textarea>
      <div class="row end" style="margin-top:8px"><button class="ghost" id="next-cancel">Cancel</button><button class="primary" id="next-save">Save</button></div>`;
    const ta = document.getElementById('next-in');
    ta.focus();
    const save = () => guard(async () => { await api('PATCH', base, { next_step: ta.value }); toast('Next step saved'); renderProject(p.slug); });
    document.getElementById('next-save').onclick = save;
    document.getElementById('next-cancel').onclick = () => { ta.value = ''; renderProject(p.slug); };
    submitOnCmdEnter(ta, save);
  };

  // Details editor
  app.querySelector('[data-edit="details"]').onclick = () => {
    const v = document.getElementById('details-view');
    v.innerHTML = `
      <div class="field"><label for="d-name">Name</label><input id="d-name" value="${esc(p.name)}"></div>
      <div class="field"><label for="d-cat">Category</label><input id="d-cat" list="cats" value="${esc(p.category)}"><datalist id="cats">${meta.categories.map(c => `<option value="${esc(c)}">`).join('')}</datalist></div>
      <div class="field"><label for="d-sum">Summary</label><textarea id="d-sum">${esc(p.summary)}</textarea></div>
      <div class="field"><label for="d-links">Links and paths <span class="hint">(one per line)</span></label><textarea id="d-links">${esc(p.links)}</textarea></div>
      <div class="row end"><button class="ghost" id="d-cancel">Cancel</button><button class="primary" id="d-save">Save</button></div>`;
    document.getElementById('d-cancel').onclick = () => { v.querySelectorAll('textarea').forEach(t => t.value = ''); renderProject(p.slug); };
    document.getElementById('d-save').onclick = () => guard(async () => {
      await api('PATCH', base, {
        name: document.getElementById('d-name').value,
        category: document.getElementById('d-cat').value,
        summary: document.getElementById('d-sum').value,
        links: document.getElementById('d-links').value,
      });
      meta = await api('GET', '/api/meta');
      toast('Saved');
      renderProject(p.slug);
    });
  };

  app.querySelectorAll('[data-logf]').forEach(b => b.onclick = () => { pref.logFilter = b.dataset.logf; savePref(); renderProject(p.slug); });

  // Notes
  const note = document.getElementById('note');
  const addNote = () => guard(async () => {
    if (!note.value.trim()) return note.focus();
    await api('POST', `${base}/notes`, { body: note.value });
    note.value = '';
    toast('Note added');
    renderProject(p.slug);
  });
  document.getElementById('add-note').onclick = addNote;
  submitOnCmdEnter(note, addNote);

  app.querySelectorAll('[data-note]').forEach(li => {
    const id = li.dataset.note;
    const n = p.notes.find(x => String(x.id) === id);
    const actions = li.querySelector('.actions');
    const del = li.querySelector('[data-act="del"]');
    del.onclick = () => {
      actions.innerHTML = `<span class="hint">Delete this?</span><button class="ghost danger" data-yes>Delete</button><button class="ghost" data-no>Keep</button>`;
      actions.querySelector('[data-no]').onclick = () => renderProject(p.slug);
      actions.querySelector('[data-yes]').onclick = () => guard(async () => { await api('DELETE', `/api/notes/${id}`); toast('Deleted'); renderProject(p.slug); });
    };
    const edit = li.querySelector('[data-act="edit"]');
    if (edit) edit.onclick = () => {
      const body = li.querySelector('.body');
      body.innerHTML = `<textarea>${esc(n.body)}</textarea><div class="row end" style="margin-top:8px"><button class="ghost" data-c>Cancel</button><button class="primary" data-s>Save</button></div>`;
      const ta = body.querySelector('textarea');
      ta.focus();
      const save = () => guard(async () => { await api('PATCH', `/api/notes/${id}`, { body: ta.value }); toast('Saved'); renderProject(p.slug); });
      body.querySelector('[data-s]').onclick = save;
      body.querySelector('[data-c]').onclick = () => { ta.value = ''; renderProject(p.slug); };
      submitOnCmdEnter(ta, save);
    };
  });

  // Tasks
  const sd = app.querySelector('[data-showdone]');
  if (sd) sd.onclick = () => { pref.showDoneTasks = !showDone; savePref(); renderProject(p.slug); };
  const to = app.querySelector('[data-taskorder]');
  if (to) to.onclick = () => { pref.newestTasksFirst = !newestFirst; savePref(); renderProject(p.slug); };
  app.querySelectorAll('[data-task]').forEach(cb => cb.onchange = () => guard(async () => {
    await api('PATCH', `/api/tasks/${cb.dataset.task}`, { done: cb.checked });
    renderProject(p.slug);
  }));
  app.querySelectorAll('[data-deltask]').forEach(b => b.onclick = () => guard(async () => {
    await api('DELETE', `/api/tasks/${b.dataset.deltask}`);
    renderProject(p.slug);
  }));
  document.getElementById('task-form').onsubmit = e => {
    e.preventDefault();
    const inp = document.getElementById('task');
    if (!inp.value.trim()) return inp.focus();
    guard(async () => {
      await api('POST', `${base}/tasks`, { text: inp.value });
      inp.value = '';
      await renderProject(p.slug);
      document.getElementById('task').focus();
    });
  };
}

// ---- new project ------------------------------------------------------------------

function renderNew() {
  const cat = pref.filter && pref.filter !== 'all' ? pref.filter : 'personal';
  app.innerHTML = `
    <div class="form-page">
      <a class="back" href="#/">← All projects</a>
      <h1>New project</h1>
      <form id="np" class="panel">
        <div class="field"><label for="n-name">Name</label><input id="n-name" required autofocus></div>
        <div class="row" style="gap:12px;align-items:flex-start">
          <div class="field" style="flex:1;min-width:140px"><label for="n-cat">Category</label><input id="n-cat" list="cats" value="${esc(cat)}"><datalist id="cats">${meta.categories.map(c => `<option value="${esc(c)}">`).join('')}</datalist></div>
          <div class="field"><label for="n-status">Status</label><select id="n-status">${meta.statuses.filter(s => !FINISHED.includes(s) || s === 'done').map(s => `<option>${s}</option>`).join('')}</select></div>
        </div>
        <div class="field"><label for="n-next">Next step</label><input id="n-next" placeholder="The very next thing to do"></div>
        <div class="field"><label for="n-sum">Summary</label><textarea id="n-sum" placeholder="What is it, and why does it matter?"></textarea></div>
        <div class="field"><label for="n-links">Links and paths <span class="hint">(one per line)</span></label><textarea id="n-links" style="min-height:60px"></textarea></div>
        <div class="row end"><a class="btn" href="#/">Cancel</a><button class="primary">Create project</button></div>
      </form>
    </div>`;
  document.getElementById('np').onsubmit = e => {
    e.preventDefault();
    guard(async () => {
      const p = await api('POST', '/api/projects', {
        name: document.getElementById('n-name').value,
        category: document.getElementById('n-cat').value,
        status: document.getElementById('n-status').value,
        next_step: document.getElementById('n-next').value,
        summary: document.getElementById('n-sum').value,
        links: document.getElementById('n-links').value,
      });
      meta = await api('GET', '/api/meta');
      location.hash = `#/p/${encodeURIComponent(p.slug)}`;
    });
  };
}

// ---- search -----------------------------------------------------------------------

async function renderSearch(q) {
  app.innerHTML = `
    <div class="form-page search-results">
      <a class="back" href="#/">← All projects</a>
      <form id="sf" class="row"><input id="sq" type="search" placeholder="Search projects, notes and tasks" value="${esc(q)}" style="flex:1;min-width:0" autofocus><button class="primary">Search</button></form>
      <div id="sr"></div>
    </div>`;
  document.getElementById('sf').onsubmit = e => {
    e.preventDefault();
    const v = document.getElementById('sq').value.trim();
    if (v) location.hash = `#/search?q=${encodeURIComponent(v)}`;
  };
  if (!q) return;
  const r = await api('GET', `/api/search?q=${encodeURIComponent(q)}`);
  const out = [];
  if (r.projects.length) out.push(`<h2>Projects</h2><div class="cards">${r.projects.map(card).join('')}</div>`);
  if (r.tasks.length) out.push(`<h2>Tasks</h2><ul class="panel feed">${r.tasks.map(t => `<li>${t.done ? '✓ ' : ''}${rich(t.text)}</li>`).join('')}</ul>`);
  if (r.notes.length) out.push(`<h2>Notes</h2><ul class="panel feed">${r.notes.map(n => `
    <li><div class="who"><a href="#/p/${encodeURIComponent(n.project_slug)}">${esc(n.project_name)}</a> · ${ago(n.created_at)}</div><div class="body">${rich(n.body)}</div></li>`).join('')}</ul>`);
  document.getElementById('sr').innerHTML = out.join('') || '<p class="empty" style="margin-top:16px">Nothing found.</p>';
}

// ---- assistant -------------------------------------------------------------------

const ask = {
  panel: document.getElementById('ask'),
  log: document.getElementById('ask-log'),
  input: document.getElementById('ask-in'),
  send: document.getElementById('ask-send'),
  session: null,
  history: [],
};
try {
  const saved = JSON.parse(localStorage.getItem('catapp-ask') || '{}');
  ask.session = saved.session || null;
  ask.history = saved.history || [];
} catch { /* fresh start */ }

function saveAsk() {
  try { localStorage.setItem('catapp-ask', JSON.stringify({ session: ask.session, history: ask.history.slice(-40) })); } catch { /* ok */ }
}

function drawAsk(pending) {
  const rows = ask.history.filter(m => m.text).map(m => `<div class="msg ${m.who}">${rich(m.text).replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')}</div>`);
  if (pending) rows.push(`<div class="msg bot pending">${pending === true ? 'Working on it…' : esc(pending)} <span id="ask-secs"></span></div>`);
  ask.log.innerHTML = rows.join('') || `<p class="hint">Ask in plain English: "add a project for learning Blender", "add a to-do to book the dentist", "what's next on my projects?", "note on Blender that I finished the donut".</p>`;
  ask.log.scrollTop = ask.log.scrollHeight;
}

function openAsk() {
  ask.panel.hidden = false;
  document.body.classList.add('asking');
  drawAsk();
  ask.input.focus();
  if (meta.voice) voiceOpen();
}
function closeAsk() {
  ask.panel.hidden = true;
  document.body.classList.remove('asking');
  if (meta.voice) { closeMic(); stopSpeaking(); }
}

async function sendAsk() {
  const text = ask.input.value.trim();
  if (!text || ask.send.disabled) return;
  if (meta.voice) return voiceSend(text);
  ask.history.push({ who: 'me', text });
  ask.input.value = '';
  ask.send.disabled = true;
  drawAsk(true);
  const t0 = Date.now();
  const timer = setInterval(() => {
    const el = document.getElementById('ask-secs');
    if (el) el.textContent = `${Math.round((Date.now() - t0) / 1000)}s`;
  }, 1000);
  try {
    const r = await api('POST', '/api/assistant', { message: text, session: ask.session });
    ask.session = r.session || ask.session;
    ask.history.push({ who: 'bot', text: r.reply || '(no reply)' });
  } catch (e) {
    ask.history.push({ who: 'bot err', text: e.message || String(e) });
  } finally {
    clearInterval(timer);
    ask.send.disabled = false;
    saveAsk();
    drawAsk();
    ask.input.focus();
    render(); // the assistant may have changed things
  }
}

document.getElementById('ask-open').onclick = openAsk;
document.getElementById('ask-open-top').onclick = openAsk;
document.getElementById('ask-close').onclick = closeAsk;
document.getElementById('ask-new').onclick = () => {
  ask.session = null; ask.history = []; saveAsk(); drawAsk(); ask.input.focus();
  if (meta.voice) { stopSpeaking(); api('POST', '/api/voice/reset').then(voiceWarm).catch(() => {}); }
};
document.getElementById('ask-form').onsubmit = e => { e.preventDefault(); sendAsk(); };
ask.input.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendAsk(); }
  if (e.key === 'Escape') { if (voice.queue.length || voice.source) stopSpeaking(); else closeAsk(); }
});

// ---- voice mode (`catapp config voice on`) --------------------------------------------
// Replies stream in from a Claude session the server keeps running while this panel is
// open; each finished sentence is spoken (Kokoro, on the server) while the next is written.
// The mic records until you stop talking, and the server transcribes it (Parakeet).

const voice = {
  ready: false,
  speak: pref.speak !== false,
  ctx: null, out: null, level: null,  // audio context, output analyser, its sample buffer
  queue: [], source: null, gen: 0,    // sentences waiting to play, the one playing
  buf: '',                            // reply text not yet spoken
  rec: null, cancelled: false, stream: null,
  lastUse: Date.now(),
  mouth: 0,
};
const mic = document.getElementById('ask-mic');
const speakBtn = document.getElementById('ask-speak');

function voiceOpen() {
  if (!voice.ready) {
    voice.ready = true;
    mic.hidden = false;
    speakBtn.hidden = false;
    speakBtn.setAttribute('aria-pressed', voice.speak);
    if (meta.voice.persona) {
      document.getElementById('ask-title').textContent = meta.voice.persona;
      document.querySelector('.ask-sub').textContent = `Claude as ${meta.voice.persona}, limited to ${meta.app}`;
    }
    if (meta.voice.avatar) loadAvatar(meta.voice.avatar).catch(e => console.warn('avatar', e));
    // Keep the session warm while the panel is open and in view, for up to 20 quiet minutes.
    setInterval(() => {
      if (!ask.panel.hidden && !document.hidden && Date.now() - voice.lastUse < 20 * 60e3) voiceWarm();
    }, 60e3);
  }
  voiceWarm();
}

function voiceWarm() {
  return api('POST', '/api/voice/warm', { session: ask.session }).catch(e => toast(e.message, true));
}

// The audio context has to start from a click or key press.
function audio() {
  if (!voice.ctx) {
    voice.ctx = new (window.AudioContext || window.webkitAudioContext)();
    voice.out = voice.ctx.createAnalyser();
    voice.out.fftSize = 512;
    voice.level = new Float32Array(voice.out.fftSize);
    voice.out.connect(voice.ctx.destination);
  }
  if (voice.ctx.state === 'suspended') voice.ctx.resume();
  return voice.ctx;
}

function rms(analyser, buf) {
  analyser.getFloatTimeDomainData(buf);
  let sum = 0;
  for (const x of buf) sum += x * x;
  return Math.sqrt(sum / buf.length);
}

function stopSpeaking() {
  voice.gen++;
  voice.queue = [];
  voice.buf = '';
  if (voice.source instanceof AudioBufferSourceNode) { try { voice.source.stop(); } catch { /* already stopped */ } }
  voice.source = null;
}

// Queue a sentence: its audio is fetched straight away, and played in order.
function say(text) {
  if (!voice.speak || !text.trim()) return;
  const gen = voice.gen;
  const clip = api('POST', '/api/voice/tts', { text }).then(async r => {
    if (!r.wav || gen !== voice.gen) return null;
    const bytes = Uint8Array.from(atob(r.wav), c => c.charCodeAt(0));
    return audio().decodeAudioData(bytes.buffer);
  }).catch(e => { toast(e.message, true); return null; });
  voice.queue.push({ clip, gen });
  if (!voice.source) playNext();
}

async function playNext() {
  const next = voice.queue.shift();
  if (!next) return;
  const claim = {}; // holds the slot while the clip loads, so sentences play one at a time
  voice.source = claim;
  const buffer = await next.clip;
  if (voice.source !== claim) return; // stopped meanwhile
  if (!buffer || next.gen !== voice.gen) { voice.source = null; return playNext(); }
  const src = audio().createBufferSource();
  src.buffer = buffer;
  src.connect(voice.out);
  src.onended = () => { if (voice.source === src) { voice.source = null; playNext(); } };
  voice.source = src;
  src.start();
}

// Speak whole sentences out of the reply so far; `final` speaks whatever's left.
function speakSentences(final) {
  const end = /[.!?…]+["')\]]*\s+|\n+/;
  let m;
  while ((m = voice.buf.match(end))) {
    const cut = m.index + m[0].length;
    say(voice.buf.slice(0, cut).trim());
    voice.buf = voice.buf.slice(cut);
  }
  if (final) { say(voice.buf.trim()); voice.buf = ''; }
}

async function voiceSend(text) {
  audio();
  stopSpeaking();
  voice.lastUse = Date.now();
  ask.history.push({ who: 'me', text });
  ask.input.value = '';
  ask.send.disabled = true;
  const bot = { who: 'bot', text: '' };
  ask.history.push(bot);
  drawAsk(true);
  let redraw = 0;
  const show = () => { cancelAnimationFrame(redraw); redraw = requestAnimationFrame(() => drawAsk(bot.text ? false : true)); };
  try {
    const r = await fetch('/api/voice/chat', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: text, session: ask.session }),
    });
    if (r.status === 401) { location.reload(); return; }
    if (!r.ok) throw new Error(((await r.json().catch(() => null)) || {}).error || r.statusText);
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let partial = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      partial += dec.decode(value, { stream: true });
      const lines = partial.split('\n');
      partial = lines.pop();
      for (const line of lines.filter(Boolean)) {
        const e = JSON.parse(line);
        if (e.t === 'text') { bot.text += e.d; voice.buf += e.d; speakSentences(false); show(); }
        else if (e.t === 'tool') speakSentences(true); // say "one sec" while it works
        else if (e.t === 'done') {
          if (e.session) ask.session = e.session;
          if (e.error) ask.history.push({ who: 'bot err', text: e.error });
        }
      }
    }
    speakSentences(true);
  } catch (e) {
    ask.history.push({ who: 'bot err', text: e.message || String(e) });
  } finally {
    cancelAnimationFrame(redraw);
    if (!bot.text) ask.history.splice(ask.history.indexOf(bot), 1);
    ask.send.disabled = false;
    saveAsk();
    drawAsk();
    if (!voice.rec) ask.input.focus();
    render(); // the assistant may have changed things
  }
}

// ---- talking: record until a second of quiet after speech (or a tap), then send ----

// The mic stays open while the panel is, so recording starts the moment you tap (opening it
// takes a moment, which used to clip the first word: "add a" came out as "I had a").
async function micStream() {
  if (!voice.stream || !voice.stream.active) {
    voice.stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } });
  }
  return voice.stream;
}
function closeMic() {
  if (voice.rec) { voice.cancelled = true; voice.rec.stop(); }
  if (voice.stream) { voice.stream.getTracks().forEach(t => t.stop()); voice.stream = null; }
}

async function startTalking() {
  audio();
  stopSpeaking();
  let stream;
  try {
    stream = await micStream();
  } catch (e) {
    toast(window.isSecureContext ? "The microphone is blocked: allow it for this page in the browser's settings."
      : 'The microphone only works over HTTPS (or on the computer running the app).', true);
    return;
  }
  const rec = new MediaRecorder(stream);
  const chunks = [];
  voice.rec = rec;
  voice.cancelled = false;
  rec.ondataavailable = e => chunks.push(e.data);
  const src = voice.ctx.createMediaStreamSource(stream);
  const an = voice.ctx.createAnalyser();
  an.fftSize = 1024;
  src.connect(an);
  const buf = new Float32Array(an.fftSize);
  const start = performance.now();
  let heard = false, quietSince = start, floor = 0, floorN = 0;
  rec.onstop = async () => {
    src.disconnect();
    voice.rec = null;
    mic.setAttribute('aria-pressed', 'false');
    mic.style.removeProperty('--level');
    if (voice.cancelled) return;
    if (!heard) { toast("Didn't catch that"); return; }
    mic.classList.add('busy');
    drawAsk('Listening…');
    try {
      const blob = new Blob(chunks, { type: rec.mimeType });
      const r = await fetch('/api/voice/stt', { method: 'POST', body: blob });
      const data = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(data.error || r.statusText);
      if (data.text) voiceSend(data.text);
      else { drawAsk(); toast("Didn't catch that"); }
    } catch (e) {
      drawAsk();
      toast(e.message || String(e), true);
    } finally {
      mic.classList.remove('busy');
    }
  };
  const tick = () => {
    if (rec.state !== 'recording') return;
    const level = rms(an, buf), now = performance.now();
    if (now - start < 300) { floor += level; floorN++; } // learn the room's background level
    const threshold = Math.max(0.012, floorN ? 3 * floor / floorN : 0);
    if (now - start >= 300 && level > threshold) { heard = true; quietSince = now; }
    mic.style.setProperty('--level', Math.min(1, level * 12).toFixed(2));
    if ((heard && now - quietSince > 1000) || (!heard && now - start > 8000) || now - start > 60e3) rec.stop();
    else requestAnimationFrame(tick);
  };
  rec.start();
  mic.setAttribute('aria-pressed', 'true');
  tick();
}

document.addEventListener('visibilitychange', () => { if (document.hidden && meta.voice) closeMic(); });

mic.onclick = () => {
  if (mic.classList.contains('busy') || ask.send.disabled) return;
  if (voice.rec) voice.rec.stop(); // tap again: send now
  else startTalking();
};
speakBtn.onclick = () => {
  voice.speak = !voice.speak;
  pref.speak = voice.speak;
  savePref();
  speakBtn.setAttribute('aria-pressed', voice.speak);
  if (!voice.speak) stopSpeaking();
};

// ---- avatar: a Live2D model whose mouth follows the voice ---------------------------

function loadScript(src) {
  return new Promise((ok, fail) => {
    const s = document.createElement('script');
    s.src = src;
    s.onload = ok;
    s.onerror = () => fail(new Error(`couldn't load ${src}`));
    document.head.appendChild(s);
  });
}

async function loadAvatar(url) {
  await loadScript('/avatar/live2dcubismcore.min.js');
  await loadScript('https://cdn.jsdelivr.net/npm/pixi.js@6.5.10/dist/browser/pixi.min.js');
  await loadScript('https://cdn.jsdelivr.net/npm/pixi-live2d-display@0.4.0/dist/cubism4.min.js');
  const el = document.getElementById('ask-avatar');
  el.hidden = false;
  document.body.classList.add('voice-avatar');
  const pixi = new PIXI.Application({ backgroundAlpha: 0, resizeTo: el, antialias: true, autoDensity: true, resolution: devicePixelRatio || 1 });
  el.appendChild(pixi.view);
  const model = await PIXI.live2d.Live2DModel.from(url);
  pixi.stage.addChild(model);
  voice.avatar = model;
  // Head and shoulders: scale so the top ~36% of the model fills the box (the sides may crop).
  const fit = () => {
    const w = el.clientWidth, h = el.clientHeight;
    const s = h / (model.internalModel.height * 0.36);
    model.scale.set(s);
    model.anchor.set(0.5, 0);
    model.position.set(w / 2, 0);
  };
  fit();
  new ResizeObserver(fit).observe(el);
  pixi.ticker.add(() => {
    const target = voice.source instanceof AudioBufferSourceNode ? Math.min(1, Math.max(0, (rms(voice.out, voice.level) - 0.01) * 9)) : 0;
    voice.mouth += (target - voice.mouth) * (target > voice.mouth ? 0.6 : 0.3);
  });
  model.internalModel.on('beforeModelUpdate', () => {
    model.internalModel.coreModel.setParameterValueById('ParamMouthOpenY', voice.mouth);
  });
}

render();
