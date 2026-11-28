import { useMemo, useState } from 'react';
import { MAX_FILE_BYTES, formatBytes } from '@duochat/protocol';
import type { ChatState } from '../lib/chatTypes';
import type { ChatEngineApi } from '../lib/engineApi';
import { filterTargets, sendTargets } from '../lib/quickSend';
import { Avatar } from './Avatar';
import { Modal } from './Modal';

/** Files dropped on the window while no chat is open: pick who gets them. */
export function DropSendDialog({
  files,
  state,
  engine,
  onClose,
  onSent,
}: {
  files: File[];
  state: ChatState;
  engine: ChatEngineApi | null;
  onClose: () => void;
  onSent: (convId: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const shown = useMemo(() => filterTargets(sendTargets(state), query), [state, query]);

  async function sendTo(convId: string) {
    if (!engine || busy) return;
    setBusy(true);
    setError(null);
    for (const file of files) {
      if (file.size > MAX_FILE_BYTES) {
        setError(`${file.name}: files can be at most ${formatBytes(MAX_FILE_BYTES)}.`);
        setBusy(false);
        return;
      }
      const bytes = new Uint8Array(await file.arrayBuffer());
      const r = await engine.sendFile(convId, { name: file.name, mime: file.type, bytes });
      if (!r.ok) {
        setError(`${file.name}: ${r.error}`);
        setBusy(false);
        return;
      }
    }
    setBusy(false);
    onSent(convId);
  }

  return (
    <Modal open={files.length > 0} onClose={onClose} title={`Send ${files.length} file${files.length === 1 ? '' : 's'} to…`}>
      <div className="drop-send">
        <input
          type="search"
          className="drop-send-search"
          aria-label="Search friends and groups"
          placeholder="Search friends and groups"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <ul className="drop-send-list">
          {shown.length === 0 ? <li className="muted small">Nobody found.</li> : null}
          {shown.map((t) => (
            <li key={t.id}>
              <button type="button" className="drop-send-row" disabled={busy} onClick={() => void sendTo(t.id)}>
                <Avatar name={t.name} size={32} group={t.group} />
                <span>{t.name}</span>
              </button>
            </li>
          ))}
        </ul>
        {error ? <p className="small" role="alert">{error}</p> : null}
      </div>
    </Modal>
  );
}
