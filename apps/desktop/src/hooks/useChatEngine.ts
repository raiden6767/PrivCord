import { useEffect, useState } from 'react';
import { watchMyServers } from '../lib/myServerLoop';
import { pushNativeConfig } from '../lib/network';
import type { ChatEngineApi } from '../lib/engineApi';
import { hostedEngineEnabled } from '../lib/hosted/enable';
import { startHostedEngine } from '../lib/hosted/start';
import type { ChatState } from '../lib/chatTypes';
import { loadIdentity } from '../lib/identity';
import { attachDiag, installDiagHooks, withDiag } from '../lib/diag';

export interface UseChatEngine {
  state: ChatState | null;
  engine: ChatEngineApi | null;
  fatal: string | null;
  /** First run: no identity yet, so the 12-word phrase flow must run first. */
  needsIdentity: boolean;
  /** Call after an identity was created or restored. */
  reload: () => void;
}

/** The previous engine going away: a new one starts only after it, so the shell never hands back the old engine. */
let stopping: Promise<void> = Promise.resolve();

export function useChatEngine(): UseChatEngine {
  const [state, setState] = useState<ChatState | null>(null);
  const [engine, setEngine] = useState<ChatEngineApi | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [needsIdentity, setNeedsIdentity] = useState(false);
  const [version, setVersion] = useState(0);

  useEffect(() => (engine ? watchMyServers(() => engine.reloadNetwork()) : undefined), [engine]);

  useEffect(() => {
    installDiagHooks();
    let cancelled = false;
    let created: ChatEngineApi | undefined;
    let unsubscribe: (() => void) | undefined;

    void (async () => {
      await stopping;
      const identity = await loadIdentity();
      if (cancelled) return;
      if (!hostedEngineEnabled()) {
        throw new Error('PrivCord runs inside its desktop app: the engine is hosted by the app shell.');
      }
      if (!identity) {
        // No recovery phrase on this device: the engine may hold an account this device was linked to, or the person
        // may link it now. Start it either way; only an engine with no account asks for an identity.
        const hosted = await startHostedEngine(null);
        created = hosted;
        if (cancelled) {
          hosted.stop();
          return;
        }
        setEngine(withDiag(hosted));
        attachDiag(hosted);
        unsubscribe = hosted.subscribe(setState);
        await hosted.start();
        setNeedsIdentity(!hosted.getState().uid);
        await pushNativeConfig();
        return;
      }
      setNeedsIdentity(false);
      const hosted = await startHostedEngine(identity);
      created = hosted;
      if (cancelled) {
        hosted.stop();
        return;
      }
      setEngine(withDiag(hosted));
      attachDiag(hosted);
      unsubscribe = hosted.subscribe(setState);
      await hosted.start();
      // The shell's relay sockets get the settings once the engine runs (and, after a start at sign-in, the hidden
      // window can then go).
      await pushNativeConfig();
    })().catch((error) => {
      if (!cancelled) setFatal(error instanceof Error ? error.message : String(error));
    });

    return () => {
      cancelled = true;
      unsubscribe?.();
      if (created && 'stopAndWait' in created) stopping = (created as { stopAndWait(): Promise<void> }).stopAndWait();
      else created?.stop();
      setEngine(null);
      setState(null);
    };
  }, [version]);

  return { state, engine, fatal, needsIdentity, reload: () => setVersion((v) => v + 1) };
}
