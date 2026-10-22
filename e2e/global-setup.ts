// The desk servers are started separately (./stack.sh) so their PIDs are known and killed by PID
// afterwards. Fail loudly if they aren't up instead of testing nothing.
export const URLS = { setup: 'http://127.0.0.1:8791', configured: 'http://127.0.0.1:8792', mainnet: 'http://127.0.0.1:8793' };

export default async function globalSetup() {
  for (const base of Object.values(URLS)) {
    const u = `${base}/api/state`;
    const r = await fetch(u).catch((e) => {
      throw new Error(`Misthos desk not running (${u}: ${e.cause?.code || e.message}). Start it with: npm run stack`);
    });
    if (!r.ok) throw new Error(`${u} answered ${r.status}; restart the stack (npm run stack:stop && npm run stack)`);
  }
}
