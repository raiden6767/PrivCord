// The admin panel. Everything shown that came from the server goes in with textContent, never as HTML.
(() => {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const store = (k) => { try { return window[k]; } catch { return null; } };
  const session = store('sessionStorage'), local = store('localStorage');
  let token = (session && session.getItem('pc-admin-token')) || '';
  let me = null, current = 'home', timer = null;

  // ---------- small helpers ----------
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
  const put = (node, ...kids) => node.replaceChildren(...kids.flat().filter((k) => k != null && k !== false));
  const say = (msg) => { $('status').textContent = ''; setTimeout(() => { $('status').textContent = msg; }, 30); };
  const when = (secs) => (secs ? new Date(secs * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' }) : 'never');
  const ago = (secs) => { if (!secs) return 'not seen yet'; const m = Math.max(0, Math.round((Date.now() / 1000 - secs) / 60)); return m < 1 ? 'seen just now' : m < 60 ? `seen ${m} min ago` : m < 1440 ? `seen ${Math.round(m / 60)} h ago` : `seen ${when(secs)}`; };
  const dur = (s) => { const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60); return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`; };
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  // Icons are drawn here, one stroke weight (no emoji or glyphs standing in for icons).
  const PATHS = {
    copy: 'M9 9h11v11H9z M5 15V4h11', check: 'M5 12.5l4.5 4.5L19 7', trash: 'M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13 M10 11v5 M14 11v5',
    alert: 'M12 3.5l9.5 16.5h-19z M12 10v4.5 M12 17.2v.1', sun: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z M12 2v2 M12 20v2 M2 12h2 M20 12h2 M5 5l1.5 1.5 M17.5 17.5L19 19 M5 19l1.5-1.5 M17.5 6.5L19 5',
    moon: 'M20 14.5A8.5 8.5 0 0 1 9.5 4 8.5 8.5 0 1 0 20 14.5z', arrow: 'M5 12h14 M13 6l6 6-6 6', plus: 'M12 5v14 M5 12h14',
  };
  function icon(name) {
    const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    s.setAttribute('viewBox', '0 0 24 24'); s.setAttribute('class', 'ic'); s.setAttribute('aria-hidden', 'true');
    const p = document.createElementNS('http://www.w3.org/2000/svg', 'path'); p.setAttribute('d', PATHS[name]); s.append(p);
    return s;
  }

  async function api(path, opts = {}) {
    const res = await fetch('/admin/api' + path, {
      method: opts.method || 'GET',
      headers: Object.assign({ Authorization: 'Bearer ' + token }, opts.body ? { 'Content-Type': 'application/json' } : {}),
      body: opts.body ? JSON.stringify(opts.body) : undefined, cache: 'no-store',
    });
    let data = null;
    try { data = await res.json(); } catch { /* not JSON */ }
    if (res.status === 401) { signOut('That token was not accepted.'); throw new Error('signed out'); }
    if (!res.ok) throw new Error((data && data.error) || `Something went wrong (${res.status}).`);
    return data;
  }

  // ---------- parts shared by the pages ----------
  function skeleton() { return el('div', { 'aria-hidden': 'true' }, el('div', { class: 'sk title' }), el('div', { class: 'sk w90' }), el('div', { class: 'sk w75' }), el('div', { class: 'sk w85' })); }
  const errorState = (msg, retry) => el('div', { class: 'err-state', role: 'alert' }, el('p', {}, msg), el('button', { class: 'btn ghost sm', type: 'button', on: { click: retry } }, 'Try again'));

  const copyTimers = new WeakMap();
  function copyRow(text, what, primary) {
    const code = el('code', {}, text), tip = el('span', { class: 'tip', role: 'status' });
    const btn = el('button', { type: 'button', class: 'btn sm ' + (primary ? 'primary' : 'ghost'), 'aria-label': `Copy ${what}` }, icon('copy'), 'Copy');
    btn.addEventListener('click', async () => {
      let msg = 'Copied';
      try { await navigator.clipboard.writeText(text); } catch {
        // No clipboard here (a page served over plain http): select the text so Ctrl+C works.
        const r = document.createRange(); r.selectNodeContents(code); const s = getSelection(); s.removeAllRanges(); s.addRange(r); msg = 'Selected. Press Ctrl+C';
      }
      clearTimeout(copyTimers.get(btn)); tip.textContent = ''; requestAnimationFrame(() => { tip.textContent = msg; });
      copyTimers.set(btn, setTimeout(() => { tip.textContent = ''; }, 3000));
    });
    return el('div', { class: 'copyrow' }, code, btn, tip);
  }

  // A button that asks before it acts, in place (no modal): the safe answer gets focus.
  function confirmAction({ label, ask, yes, onConfirm, solid, small }) {
    const wrap = el('div');
    const open = el('button', { type: 'button', class: 'btn danger' + (small === false ? '' : ' sm') }, label);
    const back = () => { put(wrap, open); open.focus(); };
    open.addEventListener('click', () => {
      const no = el('button', { type: 'button', class: 'btn ghost sm', on: { click: back } }, 'Keep it');
      const go = el('button', { type: 'button', class: 'btn danger solid sm' }, yes || 'Yes, remove');
      go.addEventListener('click', async () => {
        go.setAttribute('aria-busy', 'true'); go.disabled = true; go.textContent = 'Working…';
        try { await onConfirm(); } catch (e) { say(e.message); put(wrap, el('p', { class: 'msg bad', role: 'alert' }, e.message), open); }
      });
      put(wrap, el('div', { class: 'confirm', role: 'group', 'aria-label': 'Confirm' }, el('p', {}, ask), no, go)); no.focus();
    });
    put(wrap, open);
    return wrap;
  }

  function switchRow({ id, label, hint, checked, disabled, onChange }) {
    const btn = el('button', { type: 'button', class: 'switch', role: 'switch', id, 'aria-checked': String(!!checked), 'aria-describedby': id + '-h', disabled: !!disabled },
      el('span', { class: 'track', 'aria-hidden': 'true' }), el('span', {}, label));
    btn.addEventListener('click', async () => {
      const next = btn.getAttribute('aria-checked') !== 'true';
      btn.setAttribute('aria-checked', String(next));
      try { await onChange(next); say(`${label}: ${next ? 'on' : 'off'}`); }
      catch (e) { btn.setAttribute('aria-checked', String(!next)); say(e.message); put(hintEl, e.message); hintEl.className = 'hint msg bad'; }
    });
    const hintEl = el('p', { class: 'hint', id: id + '-h' }, hint);
    return el('div', {}, btn, hintEl);
  }

  // ---------- Advanced + theme ----------
  const html = document.documentElement;
  function setAdvanced(on, save) {
    html.setAttribute('data-advanced', on ? 'on' : 'off'); $('adv').setAttribute('aria-checked', String(on));
    if (save && local) { try { on ? local.setItem('pc-admin-advanced', '1') : local.removeItem('pc-admin-advanced'); } catch { /* not saved */ } }
  }
  setAdvanced(!!(local && local.getItem('pc-admin-advanced')), false);
  $('adv').addEventListener('click', () => { const on = $('adv').getAttribute('aria-checked') !== 'true'; setAdvanced(on, true); say(on ? 'Advanced options shown' : 'Advanced options hidden'); if (me && current === 'settings') show('settings'); });

  const meta = document.querySelector('meta[name="color-scheme"]');
  const systemDark = () => matchMedia('(prefers-color-scheme: dark)').matches;
  const showingDark = () => (meta.content === 'dark' ? true : meta.content === 'light' ? false : systemDark());
  function themeButton() {
    const toDark = !showingDark(), b = $('theme');
    b.setAttribute('aria-label', toDark ? 'Switch to dark theme' : 'Switch to light theme'); put(b, icon(toDark ? 'moon' : 'sun'));
  }
  $('theme').addEventListener('click', () => {
    const target = showingDark() ? 'light' : 'dark', system = systemDark() ? 'dark' : 'light';
    try { if (target === system) localStorage.removeItem('color-scheme'); else localStorage.setItem('color-scheme', target); } catch { /* not saved */ }
    meta.content = target === system ? 'light dark' : target; themeButton();
  });
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', themeButton);
  window.addEventListener('storage', (e) => { if (e.key === 'color-scheme') { meta.content = e.newValue === 'light' || e.newValue === 'dark' ? e.newValue : 'light dark'; themeButton(); } });
  themeButton();

  // ---------- sign in / out ----------
  function signOut(message) {
    token = ''; me = null; clearInterval(timer);
    if (session) session.removeItem('pc-admin-token');
    $('app').hidden = true; $('who').hidden = true; $('login').hidden = false;
    const err = $('loginError'); err.hidden = !message; err.textContent = message || '';
    $('token').value = ''; $('token').focus(); offerSetup();
  }
  $('signout').addEventListener('click', () => signOut(''));
  $('loginForm').addEventListener('submit', async (e) => {
    e.preventDefault(); token = $('token').value.trim();
    try { await start(); } catch (err) { if (token) signOut(err.message === 'signed out' ? '' : err.message); }
  });

  // ---------- pages ----------
  const PAGES = [
    ['home', 'Home', home], ['people', 'People', people], ['rules', 'Rules', rules, (m, o) => o.roles.nostr], ['settings', 'Settings', settings],
  ];
  let overviewCache = null;
  const tabsOf = () => [...$('tabs').querySelectorAll('[role="tab"]')];
  function buildTabs() {
    const tabs = $('tabs'); tabs.replaceChildren();
    for (const [id, label, , allowed] of PAGES) {
      if (allowed && !allowed(me, overviewCache)) continue;
      tabs.append(el('button', { role: 'tab', id: 'tab-' + id, type: 'button', 'aria-selected': String(id === current), tabindex: id === current ? '0' : '-1', 'aria-controls': 'view', on: { click: () => show(id) } }, label));
    }
    tabs.onkeydown = (e) => {
      const list = tabsOf(), i = list.indexOf(document.activeElement); if (i < 0) return;
      const to = { ArrowRight: (i + 1) % list.length, ArrowLeft: (i + list.length - 1) % list.length, Home: 0, End: list.length - 1 }[e.key];
      if (to === undefined) return; e.preventDefault(); list[to].focus(); list[to].click();
    };
  }
  function show(id) {
    current = id; clearInterval(timer);
    for (const b of tabsOf()) { const on = b.id === 'tab-' + id; b.setAttribute('aria-selected', String(on)); b.tabIndex = on ? 0 : -1; }
    $('view').setAttribute('aria-labelledby', 'tab-' + id);
    put($('view'), skeleton());
    PAGES.find((p) => p[0] === id)[2]().catch((err) => { if (err.message !== 'signed out') put($('view'), errorState(err.message, () => show(id))); });
    if (location.hash !== '#' + id) history.replaceState(null, '', '#' + id);
  }
  const go = (id) => { if (tabsOf().some((b) => b.id === 'tab-' + id)) { tabsOf().find((b) => b.id === 'tab-' + id).focus(); show(id); } };

  // ----- Home: is it healthy, and how do I connect -----
  async function home() {
    const draw = async () => {
      const o = overviewCache = await api('/overview');
      if (current !== 'home') return;
      const c = o.counters || {}, n = c.nostr || null;
      const row = (what, detail, state) => el('div', { class: 'item' }, el('span', { class: 'what' }, what), state, detail ? el('span', { class: 'detail' }, detail) : null);
      const onoff = (b) => el('span', { class: 'state' }, el('span', { class: 'dot' + (b ? ' on' : ''), 'aria-hidden': 'true' }), b ? 'On' : 'Off');
      put($('view'),
        el('div', { class: 'page-head' }, el('h2', {}, 'Home'), el('p', { class: 'lead' }, o.access.private ? 'This server is private. Only people you gave a token can connect.' : 'This server is open: anyone who finds it can connect.')),
        o.warnings.length ? el('section', { 'aria-labelledby': 'attn-h' }, el('h3', { id: 'attn-h' }, 'Needs your attention'),
          el('div', { class: 'attn' }, o.warnings.map((w) => el('div', { class: 'item' }, icon('alert'), el('span', {}, w.text), w.go ? el('button', { type: 'button', class: 'btn ghost sm act', on: { click: () => go(w.go) } }, w.go === 'people' ? 'Add a token' : w.go === 'rules' ? 'Set rules' : 'Open settings') : null)))) : null,
        el('section', {}, el('h3', {}, 'Connect your apps'),
          el('p', { class: 'muted' }, 'Add this address as the relay in each app (Settings, Connection).' + (o.access.private ? ' Add a token to the end as ?token=…' : '')),
          copyRow(o.addresses.relay, 'relay address', true)),
        o.addresses.turn ? el('section', { class: 'adv' }, el('h3', {}, 'For calls and files'),
          el('p', { class: 'muted' }, 'Used when two phones cannot reach each other directly. The first is UDP; the others get through networks that block it.'),
          o.addresses.turn.map((t) => el('div', { class: 'gap-t' }, copyRow(t, 'call address')))) : null,
        el('section', {}, el('h3', {}, 'Status'),
          el('div', { class: 'list' },
            row('Devices connected now', null, el('span', { class: 'num what' }, String(c.open_connections ?? 0))),
            row('Messages and calls (relay)', null, onoff(o.roles.relay)),
            row('Nostr', null, onoff(o.roles.nostr)),
            row('Calls through this server (TURN)', null, onoff(o.roles.turn)),
            row('Running for', `Version ${o.version}`, el('span', { class: 'num what' }, dur(o.uptime_secs))))),
        el('section', { class: 'adv' }, el('h3', {}, 'Activity since it started'),
          el('div', { class: 'list' }, [
            ['Messages received', c.put], ['Messages delivered', c.delivered], ['Messages waiting for someone offline', c.queued], ['Mailboxes holding messages', c.mailboxes],
            ['Requests refused', c.rejected], ...(n ? [['Nostr events stored', n.events], ['Nostr events delivered', n.delivered], ['Nostr events refused', n.rejected]] : []),
          ].map(([l, v]) => el('div', { class: 'item' }, el('span', {}, l), el('span', { class: 'num what' }, String(v ?? 0)))))));
      buildTabs();
    };
    await draw();
    timer = setInterval(() => { if (!$('view').contains(document.activeElement)) draw().catch(() => {}); }, 8000);
  }

  // ----- People: who can connect (and, for owners, who can manage) -----
  async function people() {
    const canChange = me.role !== 'viewer';
    const secretBox = el('div', { 'aria-live': 'polite' }), listBox = el('div'), intro = el('p', { class: 'lead' });
    const nameIn = el('input', { id: 'tname', name: 'name', maxlength: '60', required: true, autocomplete: 'off', placeholder: 'For example: Alice’s phone' });
    const nameMsg = el('p', { class: 'msg bad', hidden: true, id: 'tname-e' });
    const fail = (m) => { nameMsg.textContent = m; nameMsg.hidden = !m; m ? (nameIn.setAttribute('aria-invalid', 'true'), nameIn.setAttribute('aria-describedby', 'tname-e')) : (nameIn.removeAttribute('aria-invalid'), nameIn.removeAttribute('aria-describedby')); };
    nameIn.addEventListener('input', () => fail(''));
    async function fill() {
      const d = await api('/access-tokens');
      const open = !d.tokens.length && !d.from_settings_file;
      put(intro, open ? 'Anyone who finds this server can use it. Make a token for each person to make it private.' : 'Only people with one of these tokens can connect. Make one for each person or device, and remove it when they leave.');
      put(listBox, d.tokens.length
        ? el('div', { class: 'list' }, d.tokens.map((t) => el('div', { class: 'item' }, el('span', { class: 'what' }, t.name), el('span', { class: 'detail' }, `Made ${when(t.created)}`),
            canChange ? el('div', { class: 'act' }, confirmAction({ label: 'Remove', yes: 'Yes, remove', ask: (d.tokens.length === 1 && !d.from_settings_file) ? `“${t.name}” is the last token. Removing it opens this server to anyone.` : `Remove “${t.name}”? They are disconnected the next time they connect.`,
              onConfirm: async () => { await api('/access-tokens/' + encodeURIComponent(t.id) + ((d.tokens.length === 1 && !d.from_settings_file) ? '?open=true' : ''), { method: 'DELETE' }); say('Token removed'); fill(); } })) : null)))
        : el('div', { class: 'empty' }, el('h3', {}, 'No tokens yet'), el('p', { class: 'muted' }, 'Make the first one above, then give it to the person it is for.')),
        d.from_settings_file ? el('p', { class: 'hint' }, `${plural(d.from_settings_file, 'more token comes', 'more tokens come')} from the settings file and can’t be changed here.`) : null);
    }
    await fill();
    put($('view'),
      el('div', { class: 'page-head' }, el('h2', {}, 'People'), intro),
      canChange ? el('section', {}, el('h3', {}, 'Make a token'),
        el('form', { on: { submit: async (e) => {
          e.preventDefault(); fail('');
          try {
            const r = await api('/access-tokens', { method: 'POST', body: { name: nameIn.value } });
            put(secretBox, el('div', { class: 'secret' }, el('p', { class: 'label' }, 'Copy this now. It is shown only once.'), copyRow(r.token, 'access token', true)));
            nameIn.value = ''; say('Token made'); fill();
          } catch (err) { fail(err.message); nameIn.focus(); }
        } } }, el('div', { class: 'inline' }, el('div', { class: 'grow' }, el('label', { for: 'tname' }, 'Who is it for?'), nameIn), el('button', { class: 'btn primary', type: 'submit' }, icon('plus'), 'Make token')), nameMsg), secretBox) : null,
      el('section', {}, el('h3', {}, 'Tokens'), listBox),
      me.role === 'owner' ? el('section', { class: 'adv', id: 'admins' }) : null);
    if (me.role === 'owner') adminsSection().catch((e) => put($('admins'), errorState(e.message, () => adminsSection())));
  }
  async function adminsSection() {
    const host = $('admins'); const d = await api('/admins');
    const secretBox = el('div', { 'aria-live': 'polite' });
    const nameIn = el('input', { id: 'aname', maxlength: '60', required: true, autocomplete: 'off', placeholder: 'For example: Sam' });
    const roleIn = el('select', { id: 'arole' }, el('option', { value: 'moderator' }, 'Moderator: rules, tokens, remove events'), el('option', { value: 'viewer' }, 'Viewer: look only'), el('option', { value: 'owner' }, 'Owner: everything'));
    const msg = el('p', { class: 'msg bad', role: 'alert', hidden: true });
    put(host, el('h3', {}, 'Admins'), el('p', { class: 'muted' }, 'People who can sign in to this panel, each with their own token.'),
      el('div', { class: 'list' }, d.admins.map((a) => el('div', { class: 'item' }, el('span', { class: 'what' }, a.name), el('span', { class: 'detail' }, `${a.role}, ${ago(a.last_used)}`),
        el('div', { class: 'act' }, confirmAction({ label: 'Remove', ask: `Remove ${a.name}? Their token stops working at once.`, onConfirm: async () => { await api('/admins/' + encodeURIComponent(a.id), { method: 'DELETE' }); say('Admin removed'); adminsSection(); } }))))),
      el('form', { class: 'gap-t', on: { submit: async (e) => {
        e.preventDefault(); msg.hidden = true;
        try { const r = await api('/admins', { method: 'POST', body: { name: nameIn.value, role: roleIn.value } }); put(secretBox, el('div', { class: 'secret' }, el('p', { class: 'label' }, 'Give this token to them. It is shown only once.'), copyRow(r.token, 'admin token', true))); nameIn.value = ''; say('Admin added'); }
        catch (err) { msg.textContent = err.message; msg.hidden = false; nameIn.focus(); }
      } } }, el('div', { class: 'inline' }, el('div', { class: 'grow' }, el('label', { for: 'aname' }, 'Name'), nameIn), el('div', { class: 'grow' }, el('label', { for: 'arole' }, 'Role'), roleIn), el('button', { class: 'btn ghost', type: 'submit' }, 'Add admin')), msg), secretBox);
  }

  // ----- Rules: who may publish on the Nostr side -----
  async function rules() {
    const p = await api('/nostr/policy'); const canChange = me.role !== 'viewer';
    const listed = p.allowed_pubkeys.length > 0;
    const msg = el('p', { class: 'msg', role: 'status' });
    const allowedKeys = el('textarea', { id: 'ak', spellcheck: 'false', 'aria-describedby': 'ak-h', disabled: !canChange }, p.allowed_pubkeys.join('\n'));
    const blockedKeys = el('textarea', { id: 'bk', spellcheck: 'false', disabled: !canChange }, p.blocked_pubkeys.join('\n'));
    const kind = (id, v) => el('input', { id, value: v.join(', '), inputmode: 'numeric', pattern: '[0-9, ]*', title: 'Numbers separated by commas', disabled: !canChange });
    const allowedKinds = kind('ako', p.allowed_kinds), blockedKinds = kind('bko', p.blocked_kinds);
    const anyone = el('input', { type: 'radio', name: 'who', id: 'w-any', checked: !listed, disabled: !canChange }), only = el('input', { type: 'radio', name: 'who', id: 'w-only', checked: listed, disabled: !canChange });
    const keysBox = el('div', { class: 'field', hidden: !listed }, el('label', { for: 'ak' }, 'Public keys allowed to publish'), allowedKeys, el('p', { class: 'hint', id: 'ak-h' }, 'One per line, in hex (64 characters).'));
    for (const r of [anyone, only]) r.addEventListener('change', () => { keysBox.hidden = !only.checked; });
    const split = (t) => t.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);
    const evId = el('input', { id: 'evid', spellcheck: 'false', required: true, pattern: '[0-9a-fA-F]{64}', title: '64 hex characters', placeholder: '64 hex characters' });
    const evMsg = el('p', { class: 'msg bad', role: 'alert', hidden: true });
    put($('view'),
      el('div', { class: 'page-head' }, el('h2', {}, 'Rules'), el('p', { class: 'lead' }, 'Who may publish on this server’s Nostr side. Changes apply at once.')),
      el('form', { on: { submit: async (e) => {
        e.preventDefault(); msg.textContent = '';
        const keys = only.checked ? split(allowedKeys.value) : [];
        if (only.checked && !keys.length) { msg.className = 'msg bad'; msg.textContent = 'Add at least one public key, or choose “Anyone”.'; allowedKeys.focus(); return; }
        try { await api('/nostr/policy', { method: 'PUT', body: { allowed_pubkeys: keys, blocked_pubkeys: split(blockedKeys.value), allowed_kinds: split(allowedKinds.value).map(Number), blocked_kinds: split(blockedKinds.value).map(Number) } }); msg.className = 'msg good'; msg.textContent = 'Saved.'; say('Rules saved'); }
        catch (err) { msg.className = 'msg bad'; msg.textContent = err.message; }
      } } },
        el('fieldset', { class: 'bare' }, el('legend', { class: 'label' }, 'Who may publish'),
          el('div', { class: 'choice' }, el('label', { for: 'w-any' }, anyone, el('span', {}, 'Anyone', el('span', { class: 'hint' }, 'Best for a public relay.'))), el('label', { for: 'w-only' }, only, el('span', {}, 'Only people I list', el('span', { class: 'hint' }, 'Best for your own and your friends’ keys.'))))),
        keysBox,
        el('div', { class: 'adv' }, el('div', { class: 'field' }, el('label', { for: 'bk' }, 'Blocked public keys'), blockedKeys, el('p', { class: 'hint' }, 'One per line.')),
          el('div', { class: 'field' }, el('label', { for: 'ako' }, 'Only accept these kinds'), allowedKinds, el('p', { class: 'hint' }, 'Numbers separated by commas. Empty accepts every kind.')),
          el('div', { class: 'field' }, el('label', { for: 'bko' }, 'Refuse these kinds'), blockedKinds)),
        canChange ? el('div', { class: 'field' }, el('button', { class: 'btn primary', type: 'submit' }, 'Save rules'), msg) : el('p', { class: 'muted gap-t' }, 'You can look but not change.')),
      canChange ? el('section', { class: 'adv' }, el('h3', {}, 'Remove one event'),
        el('p', { class: 'muted' }, 'Removes a stored event for good and refuses it if anyone publishes it again.'),
        el('form', { class: 'gap-t', on: { submit: async (e) => {
          e.preventDefault(); evMsg.hidden = true;
          try { const r = await api('/nostr/events/' + encodeURIComponent(evId.value.trim()), { method: 'DELETE' }); say(r.removed ? 'Event removed' : 'It was not stored here; it is refused from now on'); evId.value = ''; }
          catch (err) { evMsg.textContent = err.message; evMsg.hidden = false; }
        } } }, el('div', { class: 'inline' }, el('div', { class: 'grow' }, el('label', { for: 'evid' }, 'Event id'), evId), el('button', { class: 'btn danger', type: 'submit' }, 'Remove event')), evMsg)) : null);
  }

  // ----- Settings: clear data first, then the few things beginners need, then Advanced -----
  let freshToken = null; // a replacement token, shown once after the page redraws
  async function settings() {
    const s = await api('/settings'); const canChange = me.role !== 'viewer';
    const mine = await api('/me'); const shown = freshToken; freshToken = null;
    const r = s.relay, n = s.nostr;
    const clearRow = (what, count, noun, path, after) => el('div', { class: 'item' }, el('span', { class: 'what' }, what),
      el('span', { class: 'detail num' }, count ? `${plural(count, noun[0], noun[1])} stored now` : 'Nothing stored right now'),
      canChange ? el('div', { class: 'act' }, count ? confirmAction({ label: 'Clear…', yes: 'Yes, clear', ask: `This removes ${plural(count, noun[0], noun[1])} for good. People who have not received them yet never will.`,
        onConfirm: async () => { const d = await api(path, { method: 'POST', body: { confirm: true } }); say(`${plural(d.removed, noun[0], noun[1])} cleared`); after(); } }) : null) : null);
    const num = (id, label, hint, value, min, max, key) => {
      const input = el('input', { id, type: 'number', inputmode: 'numeric', min: String(min), max: String(max), value: String(value), disabled: !canChange, 'data-key': key });
      return el('div', { class: 'field' }, el('label', { for: id }, label), input, el('p', { class: 'hint' }, hint));
    };
    const msg = el('p', { class: 'msg', role: 'status' });
    const fields = [];
    const add = (f) => { fields.push(f); return f; };
    put($('view'),
      el('div', { class: 'page-head' }, el('h2', {}, 'Settings')),
      el('section', { 'aria-labelledby': 'clear-h' }, el('h3', { id: 'clear-h' }, 'Clear data'),
        el('p', { class: 'muted' }, 'These cannot be undone, so each one asks first.'),
        el('div', { class: 'list' }, r ? clearRow('Stored messages', r.stored_messages, ['message', 'messages'], '/clear/messages', () => show('settings')) : null,
          n ? clearRow('Nostr events', n.stored_events, ['event', 'events'], '/clear/nostr', () => show('settings')) : null)),
      r ? el('section', {}, el('h3', {}, 'Messages'),
        switchRow({ id: 'keep', label: 'Keep messages for people who are offline', checked: r.keep_offline_messages, disabled: !canChange || !r.can_keep_offline_messages,
          hint: r.can_keep_offline_messages ? 'They arrive when the person opens the app (kept for up to 7 days). Turn off and only people online right now receive messages.' : 'Switched off in the settings file (store_and_forward = false).',
          onChange: async (v) => { await api('/settings', { method: 'PUT', body: { keep_offline_messages: v } }); } })) : null,
      el('section', { class: 'adv' }, el('h3', {}, 'Limits'),
        el('p', { class: 'muted' }, 'Protect the server from one device or address using too much. Applied at once and kept after a restart.'),
        el('form', { on: { submit: async (e) => {
          e.preventDefault(); msg.className = 'msg'; msg.textContent = '';
          const body = {}; for (const f of fields) body[f.dataset.key] = Number(f.value);
          try { await api('/settings', { method: 'PUT', body }); msg.className = 'msg good'; msg.textContent = 'Saved.'; say('Limits saved'); }
          catch (err) { msg.className = 'msg bad'; msg.textContent = err.message; }
        } } },
          r ? [add(num('l-frames', 'Messages per minute from one address', 'Every request counts. Lower it if a single address floods the server.', r.frames_per_minute, 10, 1000000, 'frames_per_minute')),
               add(num('l-conns', 'Devices per address', 'How many devices may connect from one network address (a home with many phones needs several).', r.connections_per_address, 1, 10000, 'connections_per_address')),
               add(num('l-boxes', 'Mailboxes one device may listen to', 'Each friend or group uses one.', r.mailboxes_per_connection, 1, 1000, 'mailboxes_per_connection'))] : null,
          n ? [add(num('l-ev', 'Nostr events per minute from one address', '0 turns this limit off.', n.events_per_minute, 0, 100000, 'nostr_events_per_minute')),
               add(num('l-rq', 'Nostr requests per minute from one device', '0 turns this limit off.', n.requests_per_minute, 0, 100000, 'nostr_requests_per_minute')),
               add(num('l-days', 'Keep Nostr events for (days)', '0 keeps them for ever.', n.keep_days, 0, 3650, 'nostr_keep_days'))] : null,
          canChange ? el('div', { class: 'field' }, el('button', { class: 'btn primary', type: 'submit' }, 'Save limits'), msg) : null)),
      el('section', {}, el('h3', {}, 'Your sign-in'),
        el('p', { class: 'muted' }, mine.days_left == null ? 'Your token does not expire.' : `Your token works for ${plural(mine.days_left, 'more day', 'more days')}. Replace it before then, or ask an owner for a setup code.`),
        shown ? el('div', { class: 'secret' }, el('p', { class: 'label' }, 'This is your new token. Copy it now: it is shown only once, and the old one no longer works.'), copyRow(shown, 'new token', true)) : null,
        confirmAction({ label: 'Replace my token…', yes: 'Yes, replace it', ask: 'The old token stops working at once. You will see the new one once: copy it.',
          onConfirm: async () => { const r = await api('/me/token', { method: 'POST', body: {} }); token = r.token; if (session) session.setItem('pc-admin-token', token); freshToken = r.token; say('Token replaced'); show('settings'); } })));
  }

  // ---------- first run: the setup code makes the owner, and the token is shown once, here ----------
  async function offerSetup() {
    try {
      const r = await fetch('/admin/api/setup', { cache: 'no-store' });
      if (r.ok && (await r.json()).open) {
        $('setupTitle').textContent = 'First time here?';
        $('setupHelp').textContent = 'Enter the setup code the server printed when it started. It works once, for 15 minutes. If it has run out, restart the server for a new one.';
      }
    } catch { /* the form keeps its general wording */ }
  }
  $('setupForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const err = $('setupError'); err.hidden = true;
    try {
      const res = await fetch('/admin/api/setup', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ code: $('code').value }), cache: 'no-store' });
      const data = await res.json().catch(() => null);
      if (!res.ok) throw new Error((data && data.error) || `Something went wrong (${res.status}).`);
      $('setupForm').hidden = true; $('loginForm').hidden = true;
      const go = el('button', { type: 'button', class: 'btn primary' }, 'Sign in with it');
      go.addEventListener('click', () => { $('loginForm').hidden = false; $('token').value = data.token; $('loginForm').requestSubmit(); });
      put($('setupDone'), el('div', { class: 'secret' }, el('p', { class: 'label' }, 'This is the owner token. Copy it now: it is shown only once and cannot be recovered.'), copyRow(data.token, 'owner token', true)), el('div', { class: 'gap-t' }, go));
      $('setupDone').hidden = false; go.focus();
    } catch (x) { err.textContent = x.message; err.hidden = false; $('code').focus(); }
  });

  // ---------- start ----------
  async function start() {
    me = await api('/me'); overviewCache = await api('/overview');
    if (session) session.setItem('pc-admin-token', token);
    $('login').hidden = true; $('app').hidden = false; $('who').hidden = false;
    const wanted = location.hash.slice(1);
    current = PAGES.some((p) => p[0] === wanted && (!p[3] || p[3](me, overviewCache))) ? wanted : 'home';
    buildTabs(); show(current);
  }
  window.addEventListener('hashchange', () => { const id = location.hash.slice(1); if (me && id !== current && tabsOf().some((b) => b.id === 'tab-' + id)) show(id); });
  if (token) start().catch(() => signOut('')); else { $('token').focus(); offerSetup(); }
})();
