import { startRelay } from './server.js';

const relay = await startRelay({
  port: Number(process.env.PORT ?? 8787),
  dataDir: process.env.DATA_DIR ?? './data',
  allowStoreForward: process.env.ALLOW_STORE_FORWARD === 'true',
  rateLimit: Number(process.env.RATE_LIMIT ?? 600),
  maxSubsPerConn: Number(process.env.MAX_SUBS_PER_CONN ?? 64),
  maxPerMailboxPerIp: Number(process.env.MAX_PER_MAILBOX_PER_IP ?? 100),
  trustProxy: process.env.TRUST_PROXY === 'true',
  maxQueued: Number(process.env.MAX_QUEUED ?? 50000),
  maxConnsPerIp: Number(process.env.MAX_CONNS_PER_IP ?? 20),
  maxQueuedBytes: Number(process.env.MAX_QUEUED_BYTES ?? 256 * 1024 * 1024),
  turnSecret: process.env.TURN_SECRET ?? '',
  turnUrls: (process.env.TURN_URLS ?? '').split(',').map((u) => u.trim()).filter(Boolean),
  turnTtlSec: Number(process.env.TURN_TTL_SEC ?? 3600),
});
console.log(`privcord-relay listening on :${relay.port} (store-and-forward ${process.env.ALLOW_STORE_FORWARD === 'true' ? 'on' : 'off'}, trust-proxy ${process.env.TRUST_PROXY === 'true' ? 'on' : 'off'})`);

process.on('uncaughtException', (err) => {
  console.error('privcord-relay fatal:', err && err.message ? err.message : err);
  process.exit(1);
});

let closing = false;
for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, async () => {
    if (closing) return;
    closing = true;
    await relay.close();
    process.exit(0);
  });
}
