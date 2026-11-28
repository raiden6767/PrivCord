import { useEffect, useState } from 'react';
import type { ChatState } from '../lib/chatTypes';
import type { ChatEngineApi } from '../lib/engineApi';
import { Modal } from './Modal';
import { QrCode } from './QrCode';
import { AlertIcon, CheckIcon } from './Icons';
import { DigitBoxes } from './DigitBoxes';

/**
 * Linking another device of the account (docs/design/syncengine.md §2).
 *
 * `host`: on the device holding the account key. It shows a code (QR and text) for the new device, then the 6 digits
 * both screens show; the person confirms here only if they match.
 * `join`: on the new device. It shows a QR for the phone to scan (like linking WhatsApp Web); the phone then shows six
 * digits that are typed here, so a photo of the QR is useless on its own. The older way (type the phone's code here)
 * stays as a fallback. The account key never leaves the other device.
 */
export function DeviceLinkDialog({
  open,
  mode,
  engine,
  state,
  onClose,
  onLinked,
}: {
  open: boolean;
  mode: 'host' | 'join';
  engine: ChatEngineApi | null;
  state: ChatState | null;
  onClose: () => void;
  /** The new device is now part of the account (join mode). */
  onLinked?: () => void;
}) {
  const [code, setCode] = useState<string | null>(null);
  const [typed, setTyped] = useState('');
  const [name, setName] = useState(defaultName());
  const [joined, setJoined] = useState(false);
  const [manual, setManual] = useState(false);
  const [offer, setOffer] = useState<string | null>(null);
  const [digits, setDigits] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const link = state?.deviceLink ?? null;

  useEffect(() => {
    if (!open) return;
    setCode(null);
    setTyped('');
    setJoined(false);
    setManual(false);
    setOffer(null);
    setDigits('');
    setError(null);
    if (mode === 'host' && engine?.startDeviceLink) {
      engine.startDeviceLink().then(setCode, (e: unknown) => setError(message(e)));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode]);

  useEffect(() => {
    if (open && mode === 'join' && link?.stage === 'done') onLinked?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode, link?.stage]);

  const close = () => {
    if (link?.stage !== 'done') engine?.cancelDeviceLink?.();
    onClose();
  };

  const join = async () => {
    if (!engine?.joinDeviceLink) return;
    setBusy(true);
    setError(null);
    try {
      await engine.joinDeviceLink(typed, name);
      setJoined(true);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };

  const showQr = async () => {
    if (!engine?.startScanLinkOffer) return;
    setBusy(true);
    setError(null);
    try {
      setOffer(await engine.startScanLinkOffer(name));
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };

  const submitDigits = async (d: string) => {
    if (!engine?.submitLinkCode) return;
    setBusy(true);
    setError(null);
    try {
      await engine.submitLinkCode(d);
    } catch (e) {
      setError(message(e));
    } finally {
      setDigits('');
      setBusy(false);
    }
  };

  const confirm = async (accept: boolean) => {
    if (!engine?.confirmDeviceLink) return;
    setBusy(true);
    try {
      await engine.confirmDeviceLink(accept);
      if (!accept) onClose();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };

  const stage = link?.stage;
  const ended = stage === 'done' || stage === 'failed' || stage === 'cancelled' || stage === 'expired' ? stage : null;

  return (
    <Modal open={open} onClose={close} title={mode === 'host' ? 'Link a new device' : 'Link to your other device'}>
      {error ? (
        <p className="status-text error" role="alert">
          <AlertIcon size={15} /> {error}
        </p>
      ) : null}

      {ended === 'done' ? (
        <p className="status-text">
          <CheckIcon size={15} /> {mode === 'host' ? `${link?.name ?? 'The new device'} is linked.` : 'Linked. Your chats are on their way.'}
        </p>
      ) : ended ? (
        <p className="status-text error" role="alert">
          {ended === 'expired' ? 'The code expired. Start again.' : ended === 'cancelled' ? 'Linking was cancelled.' : 'Linking did not work. Start again.'}
        </p>
      ) : mode === 'join' && stage === 'matched' && !offer ? (
        <p className="muted">Digits match. Waiting for your other device to finish…</p>
      ) : link?.stage === 'code' && link.code ? (
        <div className="link-compare">
          <p>{mode === 'host' ? `${link.name ?? 'A device'} wants to join. Does it show these digits?` : 'Look at your other device. Does it show the same digits?'}</p>
          <p className="link-digits" aria-label="Six digit code">
            {link.code.slice(0, 3)} {link.code.slice(3)}
          </p>
          {mode === 'host' ? (
            <div className="button-row">
              <button type="button" className="button primary" disabled={busy} onClick={() => void confirm(true)}>
                <CheckIcon size={15} /> They match, link it
              </button>
              <button type="button" className="button" disabled={busy} onClick={() => void confirm(false)}>
                They don't match
              </button>
            </div>
          ) : (
            <>
              <div className="button-row">
                <button type="button" className="button primary" disabled={busy} onClick={() => void submitDigits(link.code ?? '')}>
                  <CheckIcon size={15} /> They match
                </button>
                <button type="button" className="button" disabled={busy} onClick={close}>
                  They don't match, cancel
                </button>
              </div>
              <p className="muted small">Only tap this after you have looked at your other device. If the digits differ, someone else may have your code.</p>
            </>
          )}
        </div>
      ) : mode === 'host' ? (
        code ? (
          <div className="link-offer">
            <QrCode value={code} label="Code for the new device" />
            <p>On the new device, choose "Link to a device you already use" and scan or type this code:</p>
            <p className="link-code">{code}</p>
            <p className="muted small">It works once, for ten minutes. Your account key stays on this device.</p>
          </div>
        ) : !error ? (
          <p className="muted">Making a code…</p>
        ) : null
      ) : mode === 'join' && !manual && offer ? (
        stage === 'enter-code' || stage === 'wrong-code' ? (
          <div className="link-join">
            <p>
              Your phone now shows six digits. Type them here. Only someone holding your phone can.
            </p>
            <DigitBoxes value={digits} onChange={setDigits} onComplete={(d) => void submitDigits(d)} disabled={busy} label="Digits shown on your phone" />
            {stage === 'wrong-code' ? (
              <p className="status-text error" role="alert">
                <AlertIcon size={15} /> Those digits are not right. Look at your phone and try again.
              </p>
            ) : null}
          </div>
        ) : stage === 'matched' ? (
          <p className="muted">Digits match. Tap Link on your phone to finish…</p>
        ) : (
          <div className="link-offer">
            <QrCode value={offer} label="Code for your phone to scan" />
            <ol className="link-steps">
              <li>Open PrivCord on your phone.</li>
              <li>Settings › Devices › Link a new device.</li>
              <li>Scan this code.</li>
            </ol>
            <p className="muted small">Your phone will show six digits to type here. The code works once, for ten minutes.</p>
          </div>
        )
      ) : joined ? (
        <>
          <p className="muted">Waiting for your other device…</p>
          <div className="button-row">
            <button type="button" className="button" onClick={close}>
              Cancel
            </button>
          </div>
        </>
      ) : (
        <div className="link-join">
          <label className="field">
            <span>Name for this device</span>
            <input value={name} maxLength={64} onChange={(e) => setName(e.target.value)} />
          </label>
          {manual ? (
            <label className="field">
              <span>Code from your other device</span>
              <input value={typed} onChange={(e) => setTyped(e.target.value)} placeholder="Settings › Devices › Link a new device" />
            </label>
          ) : null}
          <div className="button-row">
            {manual ? (
              <button type="button" className="button primary" disabled={busy || typed.trim().length < 10} onClick={() => void join()}>
                {busy ? 'Connecting…' : 'Link'}
              </button>
            ) : (
              <button type="button" className="button primary" disabled={busy || !engine?.startScanLinkOffer} onClick={() => void showQr()}>
                {busy ? 'Connecting…' : 'Show code to scan'}
              </button>
            )}
            <button type="button" className="button" onClick={() => setManual(!manual)}>
              {manual ? 'Scan instead' : 'Type a code instead'}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

function defaultName(): string {
  const p = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  if (/Windows/i.test(p)) return 'Windows PC';
  if (/Mac/i.test(p)) return 'Mac';
  if (/Linux/i.test(p)) return 'Linux PC';
  return 'Computer';
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
