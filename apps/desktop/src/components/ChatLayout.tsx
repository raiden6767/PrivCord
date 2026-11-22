import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChatState, UiMessage } from '../lib/engine';
import type { ChatEngineApi } from '../lib/engineApi';
import type { AppSettings } from '../lib/settings';
import { withPinsFirst } from '../lib/pins';
import { MUTE_FOREVER, NO_PREFS } from '../lib/engine';
import { deliveryRows } from '../lib/deliveryDetails';
import { config } from '../config';
import { Modal } from './Modal';
import { LOCAL_ID } from '../lib/identity';
import { formatTime } from '../lib/format';
import { MAX_FILE_BYTES, formatBytes } from '@duochat/protocol';
import { Avatar, BrandMark } from './Avatar';
import { Composer } from './Composer';
import { MessageList } from './MessageList';
import { FilterPills, type ChatFilter } from './FilterPills';
import { takeRecentNotified } from '../lib/notifications';
import { SettingsDialog, type SettingsSection } from './SettingsDialog';
import { ManualConnectDialog } from './ManualConnectDialog';
import { ClearChatDialog, NukeDialog } from './KillSwitchDialogs';
import { RecoveryDialog } from './RecoveryDialogs';
import { AddFriendDialog } from './AddFriendDialog';
import { GroupDetailsDialog, NewGroupDialog } from './GroupDialogs';
import { PersonDialog } from './PersonDialog';
import { RequestDialog } from './RequestDialog';
import { ChannelsDialog } from './ChannelsDialog';
import { useSecureScreen, useUnreadTitle } from '../hooks/useWindowEffects';
import { totalUnread, whatsNewPending } from '../lib/insightInputs';
import { SealedBadge, WhatsNewBanner } from './Insights';
import { DropSendDialog } from './DropSendDialog';
import { nearbyText, receiptLine } from '../lib/receipts';
import type { DeviceReceipt } from '../lib/engineApi';
import { AlertIcon, ArrowLeftIcon, BellOffIcon, LoaderIcon, PinIcon, RefreshIcon, SettingsIcon, ShieldIcon, UserIcon, XIcon } from './Icons';

interface Row {
  id: string;
  isGroup: boolean;
  name: string;
  preview: string;
  at: number;
  unread: number;
  online: boolean;
}

function Splash({ title, text, danger }: { title: string; text: string; danger?: boolean }) {
  return (
    <main className="loading-page">
      <div className="splash">
        <BrandMark size={72} />
        {danger ? null : <LoaderIcon size={20} className="spin" />}
        <h1>{title}</h1>
        <p className="muted">{text}</p>
        {danger ? (
          <button type="button" className="button secondary" onClick={() => window.location.reload()}>
            <RefreshIcon size={16} />
            Reload
          </button>
        ) : null}
      </div>
    </main>
  );
}

export function ChatLayout({
  state,
  engine,
  settings,
  updateSettings,
  fatal,
}: {
  state: ChatState | null;
  engine: ChatEngineApi | null;
  settings: AppSettings;
  updateSettings: (patch: Partial<AppSettings>) => void;
  fatal: string | null;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<ChatFilter>('all');
  const [menuOpen, setMenuOpen] = useState(false);
  const [dialog, setDialog] = useState<
    null | 'settings' | 'add' | 'group' | 'channels' | 'details' | 'manual' | 'nuke' | 'clear' | 'recovery'
  >(null);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('profile');
  const [panic, setPanic] = useState(false);
  const [manualUid, setManualUid] = useState('');
  const [verifyUid, setVerifyUid] = useState<string | null>(null);
  const [replying, setReplying] = useState<UiMessage | null>(null);
  const [detailsFor, setDetailsFor] = useState<UiMessage | null>(null);
  const prefs = state?.prefs ?? NO_PREFS;
  const [showArchived, setShowArchived] = useState(false);
  /** The message just sent, for a few seconds: "Undo" takes it back from everyone. */
  const [undo, setUndo] = useState<{ conv: string; text: string } | null>(null);
  useEffect(() => {
    if (!undo) return;
    const t = window.setTimeout(() => setUndo(null), 6000);
    return () => window.clearTimeout(t);
  }, [undo]);
  const undoSend = () => {
    if (!undo || !state) return;
    const mine = [...(state.messages[undo.conv] ?? [])].reverse().find((m) => m.own && m.text === undo.text);
    if (mine) engine?.deleteMessage(mine.messageId, true);
    setUndo(null);
  };
  /** Per-device receipts of the message in the details dialog (empty when the engine has none). */
  const [receipts, setReceipts] = useState<DeviceReceipt[]>([]);
  useEffect(() => {
    setReceipts([]);
    if (!detailsFor || !engine?.messageReceipts) return;
    let live = true;
    void engine.messageReceipts(detailsFor.messageId).then((r) => live && setReceipts(r), () => undefined);
    return () => {
      live = false;
    };
  }, [engine, detailsFor]);
  /** Nearby hops per chat, asked of the engine while a Bluetooth banner is on screen. */
  const [hops, setHops] = useState<Record<string, number>>({});
  const nearbyNow = !!selected && !!state?.contacts.some((c) => c.uid === selected && c.conn === 'connected' && (c.path?.kind === 'bluetooth' || c.via === 'ble'));
  useEffect(() => {
    if (!nearbyNow || !engine?.meshHops) return;
    let live = true;
    const ask = () => void engine.meshHops!().then((h) => live && setHops(h), () => undefined);
    ask();
    const t = window.setInterval(ask, 5000);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, [engine, nearbyNow]);
  /** Files dropped on the window while no chat is open: who should get them. */
  const [dropped, setDropped] = useState<File[]>([]);
  useEffect(() => {
    if (selected) return;
    const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes('Files');
    const over = (e: DragEvent) => {
      if (hasFiles(e)) e.preventDefault();
    };
    const drop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      const files = Array.from(e.dataTransfer?.files ?? []);
      if (files.length > 0) setDropped(files);
    };
    window.addEventListener('dragover', over);
    window.addEventListener('drop', drop);
    return () => {
      window.removeEventListener('dragover', over);
      window.removeEventListener('drop', drop);
    };
  }, [selected]);
  const [query, setQuery] = useState('');
  const [focusId, setFocusId] = useState<string | null>(null);
  const [legacyError, setError] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const loadFile = useCallback((id: string) => engine?.getFile(id) ?? Promise.resolve(null), [engine]);

  // Panic wipe: three quick Ctrl+Shift+Backspace presses, when the user turned it on.
  useEffect(() => {
    if (!settings.panicWipe) return;
    const times: number[] = [];
    function onKey(e: KeyboardEvent) {
      if (!(e.ctrlKey && e.shiftKey && e.key === 'Backspace')) return;
      const now = Date.now();
      times.push(now);
      while (times.length && now - times[0]! > 2000) times.shift();
      if (times.length >= 3) {
        times.length = 0;
        setPanic(true);
        setDialog('nuke');
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [settings.panicWipe]);

  useEffect(() => {
    if (!engine) return;
    const update = () => engine.setVisible(document.visibilityState === 'visible' && document.hasFocus());
    update();
    window.addEventListener('focus', update);
    window.addEventListener('blur', update);
    document.addEventListener('visibilitychange', update);
    return () => {
      window.removeEventListener('focus', update);
      window.removeEventListener('blur', update);
      document.removeEventListener('visibilitychange', update);
    };
  }, [engine]);

  // Coming back to the window right after a notification (a click on it) opens the chat it was about.
  useEffect(() => {
    const onFocus = () => {
      const id = takeRecentNotified();
      if (!id || !state) return;
      const exists =
        state.contacts.some((c) => c.uid === id) ||
        state.groups.some((g) => g.groupId === id) ||
        state.channels.some((c) => c.id === id);
      if (exists) setSelected(id);
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [state]);

  useEffect(() => {
    engine?.openConversation(selected);
    setReplying(null);
  }, [engine, selected]);

  useEffect(() => {
    if (!menuOpen) return;
    const close = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [menuOpen]);

  const rows = useMemo<Row[]>(() => {
    if (!state) return [];
    const list: Row[] = [
      ...state.contacts.filter((c) => c.direct).map((c) => ({
        id: c.uid,
        isGroup: false,
        name: c.name,
        preview: c.last ? `${c.last.own ? 'You: ' : ''}${c.last.text}` : c.status,
        at: c.last?.at ?? 0,
        unread: c.unread,
        online: c.conn === 'connected',
      })),
      ...state.channels.map((c) => ({
        id: c.id,
        isGroup: true,
        name: `# ${c.name}`,
        preview: c.last ? `${c.last.own ? 'You' : (c.last.senderName ?? 'Someone')}: ${c.last.text}` : 'Public channel',
        at: c.last?.at ?? 0,
        unread: c.unread,
        online: c.conn === 'connected',
      })),
      ...state.groups.map((g) => ({
        id: g.groupId,
        isGroup: true,
        name: g.name,
        preview: g.last
          ? `${g.last.own ? 'You' : (g.last.senderName ?? 'Someone')}: ${g.last.text}`
          : g.status === 'member'
            ? `${g.members.length} members`
            : g.status === 'removed' ? 'You were removed' : 'You left',
        at: g.last?.at ?? 0,
        unread: g.unread,
        online: g.connected > 0,
      })),
    ];
    return withPinsFirst(list.sort((a, b) => b.at - a.at || a.name.localeCompare(b.name)), state?.prefs?.pins ?? []);
  }, [state]);

  const hits = useMemo(() => (engine && query.trim().length >= 2 ? engine.search(query) : []), [engine, query, state?.messages]);

  const [updated] = useState(() => whatsNewPending(config.appVersion));
  useUnreadTitle(state ? totalUnread(state) : 0);
  useSecureScreen(state?.prefs?.secureScreen === true);

  if (fatal) return <Splash danger title="PrivCord could not start" text={fatal} />;
  if (!state || state.phase === 'starting') return <Splash title="PrivCord" text="Opening your chats…" />;
  if (state.phase === 'error') return <Splash danger title="PrivCord could not start" text={state.error ?? 'Unknown error.'} />;

  const person = state.contacts.find((c) => c.uid === selected) ?? null;
  const group = state.groups.find((g) => g.groupId === selected) ?? null;
  const channel = state.channels.find((c) => c.id === selected) ?? null;
  const current = person ?? group ?? (channel ? { name: `# ${channel.name}` } : null);
  const verifyPerson = state.contacts.find((c) => c.uid === verifyUid) ?? null;
  const messages = (selected && state.messages[selected]) || [];
  const hasChats = rows.length > 0;
  const readOnly = group !== null && group.status !== 'member';

  const headerStatus = channel
    ? 'Public channel · anyone who knows the name can read this'
    : person
    ? person.status
    : group
      ? group.status === 'member'
        ? `${group.members.length} members · ${group.connected} connected`
        : group.status === 'removed' ? 'You were removed' : 'You left'
      : '';

  function open(id: string) {
    setSelected(id);
    setMenuOpen(false);
  }

  return (
    <div className={`app-shell ${current ? 'has-chat' : ''}`}>
      <aside className="sidebar">
        <header className="sidebar-head">
          <BrandMark size={30} />
          <h1>PrivCord</h1>
          <button type="button" className="icon-button" aria-label="Settings" title="Settings" onClick={() => setDialog('settings')}>
            <SettingsIcon size={18} />
          </button>
        </header>
        <div className="sidebar-actions">
          <button type="button" className="button ink" aria-label="Add a friend" onClick={() => setDialog('add')}>
            <UserIcon size={16} />
            Add a friend
          </button>
          <button
            type="button"
            className="button secondary"
            aria-label="New group"
            onClick={() => setDialog('group')}
          >
            New group
          </button>
          <button type="button" className="button secondary" aria-label="Public channels" onClick={() => setDialog('channels')}>
            Channels
          </button>
        </div>

        {(state.legacyGroups ?? []).map((g) => (
          <div key={g.oldId} className="legacy-group" role="group" aria-label={`Old group ${g.name}`}>
            <strong>{g.name}</strong>
            <p className="muted small">
              This group is from an older version and needs to be made again.
              {g.members.length > 0 ? ` It will include ${g.members.join(', ')}.` : ''}
              {g.missing > 0 ? ` ${g.missing} former member${g.missing === 1 ? ' is' : 's are'} no longer a friend.` : ''}
            </p>
            {legacyError ? <p className="small" role="alert">{legacyError}</p> : null}
            <div className="legacy-group-actions">
              <button
                type="button"
                className="button ink"
                onClick={() => {
                  engine?.recreateLegacyGroup?.(g.oldId).then(open, (e: unknown) => setError(String((e as Error)?.message ?? e)));
                }}
              >
                Make it again
              </button>
              <button
                type="button"
                className="button secondary"
                onClick={() => {
                  engine?.dismissLegacyGroup?.(g.oldId).catch((e: unknown) => setError(String((e as Error)?.message ?? e)));
                }}
              >
                Dismiss
              </button>
            </div>
          </div>
        ))}
        {hasChats ? (
          <div className="search-box">
            <input
              type="search"
              aria-label="Search messages"
              placeholder="Search messages"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        ) : null}
        {query.trim().length >= 2 ? (
          <div className="search-results" aria-label="Search results">
            {hits.length === 0 ? <p className="muted small" style={{ padding: 12 }}>Nothing found.</p> : null}
            {hits.map((h) => (
              <button
                key={h.messageId}
                type="button"
                className="search-hit"
                onClick={() => {
                  setSelected(h.convId);
                  setFocusId(h.messageId);
                  setQuery('');
                }}
              >
                <strong>
                  {h.convName} · {h.senderName}
                </strong>
                <span className="muted small">{h.text}</span>
              </button>
            ))}
          </div>
        ) : hasChats ? (
          <>
          {updated ? <WhatsNewBanner engine={engine} /> : null}
          <FilterPills value={filter} onChange={setFilter} unread={rows.reduce((n, r) => n + (r.unread > 0 ? 1 : 0), 0)} />
          <nav className="chat-list" aria-label="Chats">
            {rows.filter((r) => prefs.archived.includes(r.id) === showArchived).filter((r) => (filter === 'unread' ? r.unread > 0 : filter === 'groups' ? r.isGroup : true)).map((row) => (
              <button
                key={row.id}
                type="button"
                className={`chat-row ${row.id === selected ? 'active' : ''}`}
                onClick={() => open(row.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  engine?.togglePin?.(row.id);
                }}
              >
                <Avatar name={row.name} size={46} online={row.online} group={row.isGroup} />
                <span className="chat-row-main">
                  <span className="chat-row-top">
                    <strong>{row.name}</strong>
                    {prefs.pins.includes(row.id) ? <PinIcon size={13} className="chat-row-pin" aria-label="Pinned" /> : null}
                    {prefs.silentChats.includes(row.id) ? <BellOffIcon size={13} className="chat-row-pin" aria-label="Silent notifications" /> : null}
                    {row.at ? <span className="muted small">{formatTime(row.at)}</span> : null}
                  </span>
                  <span className="chat-row-bottom">
                    <span className="muted small chat-row-preview">{row.preview}</span>
                    {row.unread > 0 ? <span className="unread-dot" aria-label={`${row.unread} unread`}>{row.unread}</span> : null}
                  </span>
                </span>
              </button>
            ))}
            {prefs.archived.length > 0 || showArchived ? (
              <button type="button" className="button ghost small archived-toggle" onClick={() => setShowArchived(!showArchived)}>
                {showArchived ? 'Back to chats' : `Archived (${prefs.archived.length})`}
              </button>
            ) : null}
            {rows.length > 0 && !rows.some((r) => (filter === 'unread' ? r.unread > 0 : filter === 'groups' ? r.isGroup : true)) ? (
              <p className="muted small chat-list-none">{filter === 'unread' ? 'You are all caught up.' : 'No groups yet.'}</p>
            ) : null}
          </nav>
          </>
        ) : (
          <div className="sidebar-empty">
            <p className="muted small">No chats yet.</p>
          </div>
        )}
      </aside>

      <main className="chat-panel">
        {current ? (
          <>
            <header className="chat-header">
              <button type="button" className="icon-button back-button" aria-label="Back to chats" onClick={() => setSelected(null)}>
                <ArrowLeftIcon size={18} />
              </button>
              <div className="chat-header-identity">
                <Avatar name={current.name} size={40} group={!person} />
                <div>
                  <h2>
                    {current.name}
                    <SealedBadge engine={engine} kind={person ? 'direct' : group ? 'group' : 'channel'} />
                  </h2>
                  <p className="muted small" title={person?.path?.detail}>
                    <span
                      className={`status-dot ${(person ? person.conn === 'connected' : (group?.connected ?? 0) > 0) ? 'live' : ''}`}
                      aria-hidden="true"
                    />
                    {headerStatus}
                  </p>
                </div>
              </div>
              <div className="menu-wrap" ref={menuRef}>
                <button
                  type="button"
                  className="icon-button"
                  aria-label="More"
                  aria-haspopup="menu"
                  aria-expanded={menuOpen}
                  onClick={() => setMenuOpen((v) => !v)}
                >
                  <span aria-hidden="true">⋯</span>
                </button>
                {menuOpen ? (
                  <div className="menu" role="menu">
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setMenuOpen(false);
                        engine?.togglePin?.(selected!);
                      }}
                    >
                      {prefs.pins.includes(selected!) ? 'Unpin from top' : 'Pin to top'}
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setMenuOpen(false);
                        engine?.setArchived?.(selected!, !prefs.archived.includes(selected!));
                        if (!prefs.archived.includes(selected!)) setSelected(null);
                      }}
                    >
                      {prefs.archived.includes(selected!) ? 'Bring back to chats' : 'Archive chat'}
                    </button>
                    {(prefs.muteUntil[selected!] ?? 0) > Date.now() ? (
                      <button type="button" role="menuitem" onClick={() => { setMenuOpen(false); engine?.setMuteUntil?.(selected!, 0); }}>
                        Turn notifications back on
                      </button>
                    ) : (
                      <>
                        {muteChoices().map((m) => (
                          <button key={m.label} type="button" role="menuitem" onClick={() => { setMenuOpen(false); engine?.setMuteUntil?.(selected!, m.until); }}>
                            {m.label}
                          </button>
                        ))}
                      </>
                    )}
                    {engine?.setSilent ? (
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setMenuOpen(false);
                          engine.setSilent!(selected!, !prefs.silentChats.includes(selected!));
                        }}
                      >
                        {prefs.silentChats.includes(selected!) ? 'Notifications with sound' : 'Silent notifications'}
                      </button>
                    ) : null}
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setMenuOpen(false);
                        if (person) setVerifyUid(person.uid);
                        else if (channel) {
                          engine?.leaveChannel(channel.id);
                          setSelected(null);
                        } else setDialog('details');
                      }}
                    >
                      {person ? 'Friend details' : channel ? 'Leave channel' : 'Group details'}
                    </button>
                    {person ? (
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setMenuOpen(false);
                          setManualUid(person.uid);
                          setDialog('manual');
                        }}
                      >
                        Connect without internet
                      </button>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </header>

            {person && person.conn === 'connected' && (person.path?.kind === 'bluetooth' || person.via === 'ble') ? (
              <div className="banner nearby" role="status">
                {nearbyText(person.name, hops[person.uid])}
              </div>
            ) : null}

            {state.error ? (
              <div className="banner error" role="alert">
                <AlertIcon size={16} />
                <span>{state.error}</span>
                <button type="button" className="icon-button subtle" aria-label="Dismiss" onClick={() => engine?.clearNotice()}>
                  <XIcon size={15} />
                </button>
              </div>
            ) : null}
            {state.notice ? (
              <div className="banner info" role="status">
                <span>{state.notice}</span>
                <button type="button" className="icon-button subtle" aria-label="Dismiss" onClick={() => engine?.clearNotice()}>
                  <XIcon size={15} />
                </button>
              </div>
            ) : null}
            {person?.diagnosis ? (
              <div className="banner info" role="status" data-testid="diagnosis">
                <span>{person.diagnosis.message}</span>
                {person.diagnosis.actions.includes('enable-store-forward') ? (
                  <button type="button" className="btn small" onClick={() => void engine?.enableStoreForward()}>
                    Deliver via relays
                  </button>
                ) : null}
                {person.diagnosis.actions.some((a) => a === 'add-turn' || a === 'open-connection-settings') ? (
                  <button type="button" className="btn small" onClick={() => { setSettingsSection('connection'); setDialog('settings'); }}>
                    Connection settings
                  </button>
                ) : null}
              </div>
            ) : null}
            {channel ? (
              <div className="banner subtle" role="note">
                <span>Public channel: anyone who knows the name can read it. Your messages show your name and a short ID.</span>
              </div>
            ) : null}
            {state.storageBackend === 'browser' ? (
              <div className="banner subtle" role="note">
                <span>Development mode: chats are not encrypted on disk. Use the desktop app for the real thing.</span>
              </div>
            ) : null}

            <MessageList
              convId={selected ?? ''}
              messages={messages}
              earlier={(selected && state.earlier?.[selected]) || 0}
              onLoadEarlier={engine?.loadEarlier && selected ? () => engine.loadEarlier!(selected) : undefined}
              isGroup={group !== null || channel !== null}
              canReply={channel === null}
              onMute={channel ? (uid) => engine?.muteSender(uid) : undefined}
              empty={
                group
                  ? 'No messages yet. Say hello to the group.'
                  : person?.conn === 'connected'
                    ? 'Connected. Say hello.'
                    : 'Messages you write now are sent as soon as they are online.'
              }
              onRetry={(id) => void engine?.retry(id)}
              onReact={(id, emoji) => engine?.react(id, emoji)}
              onDelete={(id, everyone) => engine?.deleteMessage(id, everyone)}
              onDetails={setDetailsFor}
              focusId={focusId}
              onFocused={() => setFocusId(null)}
              onReply={setReplying}
              loadFile={loadFile}
            />

            <Modal open={detailsFor !== null} onClose={() => setDetailsFor(null)} title="Delivery details">
              {detailsFor ? (
                <div className="delivery-details">
                  <p className="muted small">Sent {new Date(detailsFor.clientCreatedAt).toLocaleString()}</p>
                  <ul className="host-list">
                    {deliveryRows(
                      detailsFor.recipients,
                      detailsFor.deliveryState,
                      (uid) => state.contacts.find((c) => c.uid === uid)?.name ?? group?.members.find((m) => m.uid === uid)?.name ?? 'Someone',
                    ).map((r) => (
                      <li key={r.name}>
                        <strong>{r.name}</strong>
                        <span className="muted small">{r.status}</span>
                      </li>
                    ))}
                  </ul>
                  {receipts.length > 0 ? (
                    <>
                      <h3 className="small receipts-title">Devices</h3>
                      <ul className="host-list receipts-list">
                        {receipts.map((r) => (
                          <li key={r.deviceId}>
                            <span className="muted small">{receiptLine(r, formatTime)}</span>
                          </li>
                        ))}
                      </ul>
                    </>
                  ) : null}
                  <p className="muted small">
                    {person?.path ? `Right now this chat is ${person.path.label.toLowerCase()}. ` : ''}
                    Messages are end-to-end encrypted on every route. A person with several devices gets the message on each; the first one to receive it counts.
                  </p>
                </div>
              ) : null}
            </Modal>

            {readOnly ? (
              <div className="composer-note">{group!.status === 'removed' ? 'You were removed from this group.' : 'You left this group.'}</div>
            ) : (
              <>
              {undo && undo.conv === selected ? (
                <div className="undo-bar" role="status">
                  <span>Sent.</span>
                  <button type="button" onClick={undoSend}>
                    Undo
                  </button>
                </div>
              ) : null}
              <Composer
                key={selected}
                disabled={state.phase !== 'ready'}
                placeholder={group ? `Message ${group.name}` : `Message ${current.name}`}
                isChannel={channel !== null}
                quickTimes={engine?.quickTimes ? () => engine.quickTimes!() : undefined}
                onSchedule={engine?.scheduleMessage ? (text, at) => engine.scheduleMessage!(selected!, text, at) : undefined}
                scheduled={(state.scheduled ?? []).filter((m) => m.target === selected)}
                onCancelScheduled={(id) => void engine?.cancelScheduled?.(id)}
                reply={replying ? { name: replying.own ? 'yourself' : replying.senderName, preview: replying.text } : null}
                onCancelReply={() => setReplying(null)}
                onSend={async (text) => {
                  const r = (await engine?.send(selected!, text, replying?.messageId)) ?? { ok: false as const, error: 'Not ready.' };
                  if (r.ok) {
                    setReplying(null);
                    setUndo({ conv: selected!, text });
                  }
                  return r;
                }}
                onSendFile={async (file) => {
                  if (file.size > MAX_FILE_BYTES) return { ok: false as const, error: `Files can be at most ${formatBytes(MAX_FILE_BYTES)}.` };
                  const bytes = new Uint8Array(await file.arrayBuffer());
                  const r = (await engine?.sendFile(selected!, { name: file.name, mime: file.type, bytes }, replying?.messageId)) ?? { ok: false as const, error: 'Not ready.' };
                  if (r.ok) setReplying(null);
                  return r;
                }}
              />
              </>
            )}
          </>
        ) : (
          <div className="welcome">
            <div className="empty-emblem duo-link" aria-hidden="true">
              <svg viewBox="0 0 160 64" width="160" height="64" fill="none">
                <path className="duo-line" d="M44 32 H116" strokeWidth="2" strokeLinecap="round" />
                <circle className="duo-dot duo-dot-a" cx="36" cy="32" r="16" />
                <circle className="duo-dot duo-dot-b" cx="124" cy="32" r="16" />
                <circle className="duo-pulse" cx="44" cy="32" r="3.5" />
              </svg>
            </div>
            <h2>{hasChats ? 'Pick a chat' : 'Your first conversation'}</h2>
            {hasChats ? (
              <p className="muted">Choose someone from the list, or add a friend.</p>
            ) : (
              <>
                <p className="muted">Swap invite codes with a friend in person or by message. Your chats go device to device; no server keeps them.</p>
                <ol className="welcome-steps">
                  <li>Click <strong>Add a friend</strong> and swap invites.</li>
                  <li>Start chatting. Messages go straight to their device.</li>
                  <li>Make a <strong>group</strong>. Friends you add can add theirs, and everyone connects automatically.</li>
                </ol>
                <button type="button" className="button primary" onClick={() => setDialog('add')}>
                  Add a friend
                </button>
              </>
            )}
          </div>
        )}
      </main>

      {dropped.length > 0 ? (
        <DropSendDialog files={dropped} state={state} engine={engine} onClose={() => setDropped([])} onSent={(id) => { setDropped([]); open(id); }} />
      ) : null}
      <RequestDialog requests={state.requests} engine={engine} onAdded={open} />
      <ChannelsDialog
        open={dialog === 'channels'}
        onClose={() => setDialog(null)}
        engine={engine}
        channels={state.channels}
        onJoined={open}
      />
      <AddFriendDialog open={dialog === 'add'} onClose={() => setDialog(null)} engine={engine} answer={state.answer} onAdded={open} />
      <NewGroupDialog open={dialog === 'group'} onClose={() => setDialog(null)} engine={engine} state={state} onCreated={open} />
      <GroupDetailsDialog
        open={dialog === 'details' && group !== null}
        onClose={() => setDialog(null)}
        engine={engine}
        state={state}
        group={group}
        onVerify={(uid) => {
          setDialog(null);
          setVerifyUid(uid);
        }}
        onMessage={(uid) => {
          setDialog(null);
          open(uid);
        }}
      />
      <PersonDialog
        open={verifyPerson !== null}
        onClose={() => setVerifyUid(null)}
        engine={engine}
        person={verifyPerson}
        onManual={(uid) => {
          setManualUid(uid);
          setDialog('manual');
        }}
        onRemoved={() => setSelected(null)}
      />
      <ManualConnectDialog open={dialog === 'manual'} onClose={() => setDialog(null)} engine={engine} state={state} uid={manualUid} />
      <SettingsDialog
        open={dialog === 'settings'}
        onClose={() => setDialog(null)}
        initialSection={settingsSection}
        engine={engine}
        state={state}
        settings={settings}
        updateSettings={updateSettings}
        onOpenNuke={() => setDialog('nuke')}
        onOpenClearChats={() => setDialog('clear')}
        onOpenRecovery={() => setDialog('recovery')}
      />
      <NukeDialog
        autoRun={panic}
        open={dialog === 'nuke'}
        onClose={() => setDialog(null)}
        engine={engine}
        uid={LOCAL_ID}
        peerUid=""
        onFinished={async () => {
          window.location.reload();
        }}
      />
      <ClearChatDialog open={dialog === 'clear'} onClose={() => setDialog(null)} engine={engine} uid={LOCAL_ID} peerUid="" />
      <RecoveryDialog open={dialog === 'recovery'} onClose={() => setDialog(null)} mode="restore" onIdentityReady={() => window.location.reload()} />
    </div>
  );
}

/** "Mute for": an hour, eight hours, until tomorrow morning, or until switched back on. */
function muteChoices(now = Date.now()): { label: string; until: number }[] {
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(8, 0, 0, 0);
  return [
    { label: 'Mute for 1 hour', until: now + 3_600_000 },
    { label: 'Mute for 8 hours', until: now + 8 * 3_600_000 },
    { label: 'Mute until tomorrow', until: tomorrow.getTime() },
    { label: 'Mute until I turn it on', until: MUTE_FOREVER },
  ];
}
