import { useEffect, useMemo, useState } from 'react';
import type { ChatState } from '../lib/chatTypes';
import type { ChatEngineApi } from '../lib/engineApi';
import { formatBytes } from '../lib/format';
import { storageByChat, type ChatStorage } from '../lib/storage';

/** What each chat takes on this device, with a way to drop a chat's photos and files to free space. */
export function StorageByChat({ state, engine }: { state: ChatState; engine: ChatEngineApi | null }) {
  const [confirm, setConfirm] = useState<string | null>(null);
  const nameOf = (id: string) =>
    state.contacts.find((c) => c.uid === id)?.name ?? state.groups.find((g) => g.groupId === id)?.name ?? (id.startsWith('chan:') ? `# ${id.slice(5)}` : 'Chat');
  // The packaged app asks the shared engine what each chat holds; the browser-only development engine counts in place.
  const local = useMemo(() => storageByChat(state.messages, nameOf), [state]); // eslint-disable-line react-hooks/exhaustive-deps
  const [counted, setCounted] = useState<ChatStorage[] | null>(null);
  useEffect(() => {
    let live = true;
    if (!engine?.storageByChat) return;
    void engine.storageByChat().then((r) => live && setCounted(r.map((x) => ({ ...x, name: nameOf(x.id) }))));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [engine, state]);
  const rows = (engine?.storageByChat ? (counted ?? []) : local).slice(0, 12);
  if (rows.length === 0) return null;
  const biggest = rows[0]!.bytes || 1;
  return (
    <div className="card-box">
      <strong>Storage by chat</strong>
      <ul className="storage-list">
        {rows.map((r) => (
          <li key={r.id}>
            <div className="storage-top">
              <span>{r.name}</span>
              <span className="muted small">{formatBytes(r.bytes)}</span>
            </div>
            <div className="checkup-bar" role="presentation">
              <span style={{ width: `${Math.max(3, (r.bytes / biggest) * 100)}%` }} />
            </div>
            {r.files > 0 ? (
              confirm === r.id ? (
                <div className="button-row">
                  <button
                    type="button"
                    className="button danger small"
                    onClick={() => {
                      for (const id of r.fileIds) engine?.deleteMessage(id, false);
                      setConfirm(null);
                    }}
                  >
                    Delete {r.files} file message{r.files === 1 ? '' : 's'} for me
                  </button>
                  <button type="button" className="button ghost small" onClick={() => setConfirm(null)}>
                    Keep
                  </button>
                </div>
              ) : (
                <button type="button" className="button ghost small" onClick={() => setConfirm(r.id)}>
                  Free {formatBytes(r.fileBytes)} of photos and files
                </button>
              )
            ) : null}
          </li>
        ))}
      </ul>
      <p className="muted small">Deleting removes those messages from this device only. Your friends keep their copies.</p>
    </div>
  );
}
