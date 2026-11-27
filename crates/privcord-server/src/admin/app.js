// The admin panel. Everything shown that came from the server goes in with textContent, never as HTML.
(() => {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const store = (() => { try { return window.sessionStorage; } catch { return null; } })();
  let token = (store && store.getItem('pc-admin-token')) || '';
  let me = null;
  let timer = null;

  // ---- small helpers ----
  function el(tag, attrs, ...kids) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k === 'class') n.className = v;
      else if (k === 'on') for (const [e, f] of Object.entries(v)) n.addEventListener(e, f);
      else if (v === true) n.setAttribute(k, '');
      else if (v !== false && v != null) n.setAttribute(k, v);
    }
    for (const kid of kids.flat()) if (kid != null && kid !== false) n.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    return n;
  }
  // Like replaceChildren, but optional (null) parts are left out instead of being shown as the text "null".
  const put = (node, ...kids) => node.replaceChildren(...kids.flat().filter((k) => k != null && k !== false));
  // A polite announcement for screen readers (cleared first so repeating the same message is still read out).
  const say = (msg) => { $('status').textContent = ''; setTimeout(() => { $('status').textContent = msg; }, 30); };
  const when = (secs) => (secs ? new Date(secs * 1000).toLocaleString() : 'never');
  const dur = (s) => { const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60); return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m ${s % 60}s`; };

  async function api(path, opts = {}) {
    const res = await fetch('/admin/api' + path, {
      method: opts.method || 'GET',
      headers: Object.assign({ Authorization: 'Bearer ' + token }, opts.body ? { 'Content-Type': 'application/json' } : {}),
      body: opts.body ? JSON.stringify(opts.body) : undefined,
      cache: 'no-store',
    });
    let data = null;
    try { data = await res.json(); } catch { /* not JSON */ }
    if (res.status === 401) { signOut('That token was not accepted.'); throw new Error('signed out'); }
    if (!res.ok) throw new Error((data && data.error) || `Something went wrong (${res.status}).`);
    return data;
  }

  // ---- errors shown next to the field they are about, and tied to it for screen readers ----
  let errCount = 0;
  function errorFor(input) {
    const p = el('p', { class: 'error', id: `err-${++errCount}`, hidden: true });
    const set = (msg) => {
      p.textContent = msg || ''; p.hidden = !msg;
      if (msg) { input.setAttribute('aria-invalid', 'true'); input.setAttribute('aria-describedby', p.id); }
      else { input.removeAttribute('aria-invalid'); input.removeAttribute('aria-describedby'); }
    };
    input.addEventListener('input', () => { if (!p.hidden) set(''); });
    return [p, set];
  }

  // ---- copying: say when it worked, say when it did not, and always leave a manual way ----
  const copyTimers = new WeakMap();
  function report(btn, tip, msg) {
    clearTimeout(copyTimers.get(btn));
    tip.textContent = '';
    requestAnimationFrame(() => { tip.textContent = msg; });
    copyTimers.set(btn, setTimeout(() => { tip.textContent = ''; }, 3000));
  }
  async function copyText(text, codeEl, btn, tip) {
    try {
      await navigator.clipboard.writeText(text);
      report(btn, tip, 'Copied');
    } catch {
      // No clipboard access (a page served over plain http, or permission denied): select the text so Ctrl+C works.
      const r = document.createRange(); r.selectNodeContents(codeEl);
      const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r);
      report(btn, tip, 'Selected. Press Ctrl+C');
    }
  }
  function addr(text, what) {
    const code = el('code', {}, text);
    const tip = el('span', { class: 'tip', role: 'status' });
    const btn = el('button', { type: 'button', 'aria-label': `Copy ${what || 'address'}` }, 'Copy');
    btn.addEventListener('click', () => copyText(text, code, btn, tip));
    return el('div', { class: 'addr' }, code, btn, tip);
  }

  // ---- theme: follow the system; the button picks the opposite of what is showing, and undoing it returns to the system ----
  const meta = document.querySelector('meta[name="color-scheme"]');
  const systemDark = () => matchMedia('(prefers-color-scheme: dark)').matches;
  const showingDark = () => (meta.content === 'dark' ? true : meta.content === 'light' ? false : systemDark());
  function themeLabel() {
    const toDark = !showingDark();
    $('theme').textContent = toDark ? 'Dark theme' : 'Light theme';
    $('theme').setAttribute('aria-label', toDark ? 'Switch to dark theme' : 'Switch to light theme');
  }
  $('theme').addEventListener('click', () => {
    const target = showingDark() ? 'light' : 'dark';
    const system = systemDark() ? 'dark' : 'light';
    try { if (target === system) localStorage.removeItem('color-scheme'); else localStorage.setItem('color-scheme', target); } catch { /* not saved */ }
    meta.content = target === system ? 'light dark' : target;
    themeLabel();
  });
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', themeLabel);
  window.addEventListener('storage', (e) => {
    if (e.key !== 'color-scheme') return;
    meta.content = e.newValue === 'light' || e.newValue === 'dark' ? e.newValue : 'light dark'; themeLabel();
  });
  themeLabel();

  // ---- sign in / out ----
  function signOut(message) {
    token = ''; me = null; clearInterval(timer);
    if (store) store.removeItem('pc-admin-token');
    $('app').hidden = true; $('who').hidden = true; $('login').hidden = false;
    const err = $('loginError'); err.hidden = !message; err.textContent = message || '';
    $('token').value = ''; $('token').focus();
  }
  $('signout').addEventListener('click', () => signOut(''));
  $('loginForm').addEventListener('submit', async (e) => {
    e.preventDefault(); token = $('token').value.trim();
    try { await start(); } catch (err) { if (token) signOut(err.message === 'signed out' ? '' : err.message); }
  });

  // ---- sections (tabs) ----
  const SECTIONS = [
    ['overview', 'Overview', renderOverview],
    ['access', 'Access', renderAccess],
    ['admins', 'Admins', renderAdmins, (m) => m.role === 'owner'],
    ['nostr', 'Nostr rules', renderNostr],
    ['turn', 'TURN', renderTurn],
  ];
  let current = 'overview';
  const visibleTabs = () => [...$('tabs').querySelectorAll('[role="tab"]')];

  function buildTabs() {
    const tabs = $('tabs'); tabs.replaceChildren();
    for (const [id, label, , allowed] of SECTIONS) {
      if (allowed && !allowed(me)) continue;
      tabs.append(el('button', { role: 'tab', id: 'tab-' + id, type: 'button', 'aria-selected': String(id === current), tabindex: id === current ? '0' : '-1', 'aria-controls': 'view', on: { click: () => show(id) } }, label));
    }
    tabs.onkeydown = (e) => {
      const list = visibleTabs(); const i = list.indexOf(document.activeElement);
      if (i < 0) return;
      const to = { ArrowRight: (i + 1) % list.length, ArrowLeft: (i + list.length - 1) % list.length, Home: 0, End: list.length - 1 }[e.key];
      if (to === undefined) return;
      e.preventDefault(); list[to].focus(); list[to].click(); // arrow keys move and select, as a tab list should
    };
  }
  function show(id) {
    current = id; clearInterval(timer);
    for (const b of visibleTabs()) { const on = b.id === 'tab-' + id; b.setAttribute('aria-selected', String(on)); b.tabIndex = on ? 0 : -1; }
    $('view').setAttribute('aria-labelledby', 'tab-' + id);
    const section = SECTIONS.find((s) => s[0] === id);
    put($('view'), el('p', { class: 'muted' }, 'Loading…'));
    section[2]().catch((err) => { if (err.message !== 'signed out') put($('view'), el('p', { class: 'error', role: 'alert' }, err.message)); });
    if (location.hash !== '#' + id) history.replaceState(null, '', '#' + id);
  }

  async function renderOverview() {
    const draw = async () => {
      const o = await api('/overview');
      if (current !== 'overview') return;
      const on = (b) => el('span', { class: 'pill' + (b ? ' on' : '') }, b ? 'On' : 'Off');
      const stat = (n, l) => el('div', { class: 'stat' }, el('div', { class: 'n' }, n), el('div', { class: 'l' }, l));
      const c = o.counters || {}, n = c.nostr || null;
      put($('view'),
        ...o.warnings.map((w) => el('div', { class: 'alert', role: 'note' }, w)),
        el('section', { class: 'card' },
          el('h2', {}, 'What is running'),
          el('p', { class: 'muted' }, `Version ${o.version}, up ${dur(o.uptime_secs)}.`),
          el('div', { class: 'grid' },
            el('div', { class: 'stat' }, el('div', { class: 'l' }, 'Relay'), on(o.roles.relay)),
            el('div', { class: 'stat' }, el('div', { class: 'l' }, 'Nostr'), on(o.roles.nostr)),
            el('div', { class: 'stat' }, el('div', { class: 'l' }, 'TURN'), on(o.roles.turn)),
            el('div', { class: 'stat' }, el('div', { class: 'l' }, 'Who can connect'), el('span', { class: 'pill' + (o.access.private ? ' on' : '') }, o.access.private ? 'Token holders only' : 'Anyone')))),
        el('section', { class: 'card' },
          el('h2', {}, 'Connect your apps'),
          el('p', { class: 'muted' }, 'Add these in the app’s relay settings. If you made access tokens, add one to the relay address as ?token=…'),
          o.roles.relay || o.roles.nostr ? el('div', {}, el('h3', {}, 'Relay'), addr(o.addresses.relay, 'relay address')) : null,
          o.addresses.turn ? el('div', {}, el('h3', {}, 'TURN (voice and files when phones cannot connect directly)'), ...o.addresses.turn.map((t) => addr(t, 'TURN address'))) : null),
        el('section', { class: 'card' },
          el('h2', {}, 'Activity since the server started'),
          el('div', { class: 'grid counts' },
            stat(c.connections ?? 0, 'connections'), stat(c.mailboxes ?? 0, 'mailboxes holding messages'), stat(c.queued ?? 0, 'messages waiting for their recipient'),
            stat(c.delivered ?? 0, 'messages delivered'), stat(c.put ?? 0, 'messages received'), stat(c.rejected ?? 0, 'requests refused'),
            n ? stat(n.events, 'Nostr events stored') : null, n ? stat(n.delivered, 'Nostr events delivered') : null, n ? stat(n.rejected, 'Nostr events refused') : null)));
    };
    await draw(); timer = setInterval(() => draw().catch(() => {}), 5000);
  }

  async function renderAccess() {
    const canChange = me.role !== 'viewer';
    const intro = el('div'); const listBox = el('div');
    const secretBox = el('div');
    const nameIn = el('input', { id: 'tname', name: 'name', maxlength: '60', required: true, autocomplete: 'off', placeholder: 'For example: Alice’s phone' });
    const [nameErr, setNameErr] = errorFor(nameIn);
    // The list is redrawn on its own, so a freshly made token stays on screen while the list updates.
    async function fill() {
      const d = await api('/access-tokens');
      put(intro,
        el('p', {}, d.tokens.length || d.from_settings_file
          ? 'This server is private: apps must present one of these tokens.'
          : 'This server is open: anyone who finds it can use it. Make a token to turn that off.'),
        d.from_settings_file ? el('p', { class: 'muted' }, `${d.from_settings_file} more token(s) come from the settings file and cannot be changed here.`) : null);
      put(listBox, d.tokens.length
        ? el('div', { class: 'scroll' }, el('table', {}, el('thead', {}, el('tr', {}, el('th', { scope: 'col' }, 'Name'), el('th', { scope: 'col' }, 'Made'), el('th', { scope: 'col' }, el('span', { class: 'sr-status' }, 'Actions')))),
            el('tbody', {}, d.tokens.map((t) => el('tr', {}, el('td', {}, t.name), el('td', {}, when(t.created)),
              el('td', {}, canChange ? el('button', { class: 'danger', type: 'button', 'aria-label': `Remove token ${t.name}`, on: { click: async () => {
                const last = d.tokens.length === 1 && !d.from_settings_file;
                const ask = last
                  ? `“${t.name}” is the last token. Removing it opens this server to ANYONE. Remove it anyway?`
                  : `Remove “${t.name}”? Anyone using it is disconnected next time they connect.`;
                if (!confirm(ask)) return;
                try { await api('/access-tokens/' + encodeURIComponent(t.id) + (last ? '?open=true' : ''), { method: 'DELETE' }); say('Token removed'); fill(); } catch (e) { say(e.message); }
              } } }, 'Remove') : null))))))
        : el('p', { class: 'muted' }, 'No tokens made here yet.'));
    }
    await fill();
    put($('view'),
      el('section', { class: 'card' }, el('h2', {}, 'Access'), intro, listBox),
      canChange ? el('section', { class: 'card' }, el('h3', {}, 'Make a token'),
        el('p', { class: 'muted' }, 'One per person or device, so you can remove one without affecting the others. The token is shown once.'),
        el('form', { on: { submit: async (e) => {
          e.preventDefault(); setNameErr('');
          try {
            const r = await api('/access-tokens', { method: 'POST', body: { name: nameIn.value } });
            put(secretBox, el('div', { class: 'secret' }, el('strong', {}, 'Copy this now. It will not be shown again.'), addr(r.token, 'access token')));
            nameIn.value = ''; say('Token made'); fill();
          } catch (err) { setNameErr(err.message); nameIn.focus(); }
        } } },
          el('div', { class: 'row' }, el('div', {}, el('label', { for: 'tname' }, 'Name'), nameIn, nameErr),
            el('button', { class: 'primary', type: 'submit' }, 'Make token')), el('div', { 'aria-live': 'polite' }, secretBox))) : null);
  }

  async function renderAdmins() {
    const d = await api('/admins');
    const secretBox = el('div');
    const nameIn = el('input', { id: 'aname', name: 'name', maxlength: '60', required: true, autocomplete: 'off', placeholder: 'For example: Sam' });
    const [nameErr, setNameErr] = errorFor(nameIn);
    const roleIn = el('select', { id: 'arole', name: 'role' }, el('option', { value: 'moderator' }, 'Moderator: change rules, tokens, remove events'),
      el('option', { value: 'viewer' }, 'Viewer: look only'), el('option', { value: 'owner' }, 'Owner: everything, including admins'));
    put($('view'),
      el('section', { class: 'card' }, el('h2', {}, 'Admins'),
        el('p', { class: 'muted' }, 'People who can sign in to this panel. Each has their own token, so you can remove one without affecting the others.'),
        el('div', { class: 'scroll' }, el('table', {}, el('thead', {}, el('tr', {}, el('th', { scope: 'col' }, 'Name'), el('th', { scope: 'col' }, 'Role'), el('th', { scope: 'col' }, 'Last seen'), el('th', { scope: 'col' }, el('span', { class: 'sr-status' }, 'Actions')))),
          el('tbody', {}, d.admins.map((a) => el('tr', {}, el('td', {}, a.name), el('td', {}, el('span', { class: 'pill' }, a.role)), el('td', {}, when(a.last_used)),
            el('td', {}, el('button', { class: 'danger', type: 'button', 'aria-label': `Remove admin ${a.name}`, on: { click: async () => {
              if (!confirm(`Remove ${a.name}? Their token stops working at once.`)) return;
              try { await api('/admins/' + encodeURIComponent(a.id), { method: 'DELETE' }); say('Admin removed'); renderAdmins(); } catch (e) { say(e.message); }
            } } }, 'Remove')))))))),
      el('section', { class: 'card' }, el('h3', {}, 'Add an admin'),
        el('form', { on: { submit: async (e) => {
          e.preventDefault(); setNameErr('');
          try {
            const r = await api('/admins', { method: 'POST', body: { name: nameIn.value, role: roleIn.value } });
            put(secretBox, el('div', { class: 'secret' }, el('strong', {}, 'Give this token to them. It will not be shown again.'), addr(r.token, 'admin token')));
            nameIn.value = ''; say('Admin added');
          } catch (err) { setNameErr(err.message); nameIn.focus(); }
        } } },
          el('div', { class: 'row' }, el('div', {}, el('label', { for: 'aname' }, 'Name'), nameIn, nameErr), el('div', {}, el('label', { for: 'arole' }, 'Role'), roleIn),
            el('button', { class: 'primary', type: 'submit' }, 'Add admin')), el('div', { 'aria-live': 'polite' }, secretBox))));
  }

  async function renderNostr() {
    let p;
    try { p = await api('/nostr/policy'); } catch (e) {
      put($('view'), el('section', { class: 'card' }, el('h2', {}, 'Nostr rules'), el('p', { class: 'muted' }, 'The Nostr side is switched off in the settings file ([nostr] enabled = true to use it).'))); return;
    }
    const canChange = me.role !== 'viewer';
    const allowedKeys = el('textarea', { id: 'ak', spellcheck: 'false', disabled: !canChange }, p.allowed_pubkeys.join('\n'));
    const blockedKeys = el('textarea', { id: 'bk', spellcheck: 'false', disabled: !canChange }, p.blocked_pubkeys.join('\n'));
    const kindsAttrs = { inputmode: 'numeric', pattern: '[0-9, ]*', title: 'Numbers separated by commas', disabled: !canChange };
    const allowedKinds = el('input', Object.assign({ id: 'ako', value: p.allowed_kinds.join(', ') }, kindsAttrs));
    const blockedKinds = el('input', Object.assign({ id: 'bko', value: p.blocked_kinds.join(', ') }, kindsAttrs));
    const evId = el('input', { id: 'evid', spellcheck: 'false', required: true, pattern: '[0-9a-fA-F]{64}', title: '64 hex characters', placeholder: '64 hex characters' });
    const [saveErr, setSaveErr] = errorFor(allowedKeys);
    const [evErr, setEvErr] = errorFor(evId);
    const saved = el('p', { role: 'status' });
    const split = (t) => t.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);
    put($('view'),
      el('section', { class: 'card' }, el('h2', {}, 'Nostr rules'),
        el('p', { class: 'muted' }, 'Who may publish what. Saved here, they take effect at once and survive a restart.'),
        el('form', { on: { submit: async (e) => {
          e.preventDefault(); setSaveErr(''); saved.textContent = '';
          try {
            await api('/nostr/policy', { method: 'PUT', body: {
              allowed_pubkeys: split(allowedKeys.value), blocked_pubkeys: split(blockedKeys.value),
              allowed_kinds: split(allowedKinds.value).map(Number), blocked_kinds: split(blockedKinds.value).map(Number) } });
            saved.textContent = 'Saved.'; say('Rules saved');
          } catch (err) { setSaveErr(err.message); }
        } } },
          el('label', { for: 'ak' }, 'Only these authors may publish'), allowedKeys, saveErr,
          el('p', { class: 'hint' }, 'Public keys in hex, one per line. Leave empty to let anyone publish.'),
          el('label', { for: 'bk' }, 'These authors are blocked'), blockedKeys,
          el('label', { for: 'ako' }, 'Only these kinds are accepted'), allowedKinds,
          el('p', { class: 'hint' }, 'Numbers separated by commas. Leave empty to accept every kind.'),
          el('label', { for: 'bko' }, 'These kinds are refused'), blockedKinds,
          canChange ? el('p', {}, el('button', { class: 'primary', type: 'submit' }, 'Save rules')) : el('p', { class: 'muted' }, 'You can look but not change.'), saved)),
      canChange ? el('section', { class: 'card' }, el('h3', {}, 'Remove an event'),
        el('p', { class: 'muted' }, 'Removes a stored event for good and refuses it if anyone publishes it again.'),
        el('form', { on: { submit: async (e) => {
          e.preventDefault(); setEvErr('');
          if (!confirm('Remove this event for good?')) return;
          try { const r = await api('/nostr/events/' + encodeURIComponent(evId.value.trim()), { method: 'DELETE' }); say(r.removed ? 'Event removed' : 'It was not stored here; it is refused from now on'); evId.value = ''; }
          catch (err) { setEvErr(err.message); }
        } } },
          el('div', { class: 'row' }, el('div', {}, el('label', { for: 'evid' }, 'Event id'), evId, evErr), el('button', { class: 'danger', type: 'submit' }, 'Remove event')))) : null);
  }

  async function renderTurn() {
    const o = await api('/overview');
    const t = o.addresses.turn;
    put($('view'), el('section', { class: 'card' }, el('h2', {}, 'TURN'),
      el('p', { class: 'muted' }, 'When two phones cannot connect directly (different mobile networks, strict routers) their voice and files go through this server.'),
      t ? el('div', {}, el('h3', {}, 'Addresses the apps are given'), ...t.map((x) => addr(x, 'TURN address')),
          el('p', { class: 'hint' }, 'The first is UDP. The ones marked transport=tcp and turns: are for networks that block UDP.'),
          el('p', {}, 'Credentials last an hour and are handed out automatically; there is nothing to set up per person.'))
        : el('p', {}, 'TURN is switched off. Set [turn] enabled = true in the settings file.'),
      el('p', { class: 'hint' }, 'Remember to open the UDP port range and the TCP ports in your firewall or router. Run “privcord-server doctor” on the server to check.')));
  }

  // ---- start ----
  async function start() {
    me = await api('/me');
    if (store) store.setItem('pc-admin-token', token);
    $('login').hidden = true; $('app').hidden = false; $('who').hidden = false;
    $('whoText').textContent = me.name === me.role ? me.role : `${me.name} · ${me.role}`;
    const wanted = location.hash.slice(1);
    current = SECTIONS.some((s) => s[0] === wanted && (!s[3] || s[3](me))) ? wanted : 'overview';
    buildTabs(); show(current);
  }
  window.addEventListener('hashchange', () => { const id = location.hash.slice(1); if (me && SECTIONS.some((s) => s[0] === id && (!s[3] || s[3](me))) && id !== current) show(id); });
  if (token) start().catch(() => signOut(''));
  else $('token').focus();
})();
