// ws-transport.ts — optional loopback WebSocket transport for the live fleet stream.
// Frames are NDJSON-shaped JSON (one object per text frame), exactly like the daemon's
// WebSocket wire: `hello` (full §1.3 snapshot), `snapshot` (full snapshot) and the additive
// `fleet` frame (`{event:'fleet', fleet:<backend fleet view>}`). The fleet view is rendered
// verbatim — never re-derived in TypeScript. One Rust derivation (C3) is the truth.
//
// Browsers cannot set `Authorization`/`Host` and always send `Origin`, so a strict daemon
// handshake is not reachable from a webview. This transport therefore carries the token as
// a query parameter for a browser/demo relay; the Tauri app keeps `listen('snapshot')` as
// primary and only adds WS when a credential is explicitly supplied by the bridge.
// Nothing here touches React state — it only emits normalized snapshots / fleet views.
import type { Snapshot } from './bridge';

export const FLEET_EVENT = 'fleet' as const;
/** DOM event that carries a backend-derived fleet view on the WS path (Tauri uses its `fleet` event). */
export const FLEET_STATE_EVENT = 'fleet-state' as const;

export type FleetStatus = 'connecting' | 'live' | 'reconnecting' | 'offline';

export interface WsCredentials {
  port: number;
  token?: string;
}

export interface WsOptions {
  url: string;
  token?: string;
  onFleet: (snapshot: Snapshot) => void;
  onFleetState?: (fleet: unknown) => void;
  onStatus?: (status: FleetStatus) => void;
}

/** Read optional browser/relay credentials: `?token=…&wsPort=…` (default 7407). */
export function credentialsFromQuery(): WsCredentials | null {
  if (typeof window === 'undefined') return null;
  const p = new URLSearchParams(window.location.search);
  const token = p.get('token');
  if (!token) return null;
  const rawPort = p.get('wsPort');
  const port = rawPort ? Number(rawPort) : 7407;
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
  return { port, token };
}

/** `ws://127.0.0.1:<port>/?token=…` — token in query for browser/mock relays. */
export function buildWsUrl({ port, token }: WsCredentials): string {
  const q = token ? `?token=${encodeURIComponent(token)}` : '';
  return `ws://127.0.0.1:${port}/${q}`;
}

const isSnapshot = (v: unknown): v is Snapshot =>
  !!v && typeof v === 'object'
  && Array.isArray((v as Snapshot).sessions)
  && Array.isArray((v as Snapshot).supervisors);

/** Parse one WS text frame: a Snapshot for the snapshot stream, or a raw fleet view. */
export function normalizeWsFrame(raw: string, onFleetState?: (fleet: unknown) => void): Snapshot | null {
  let f: unknown;
  try { f = JSON.parse(raw); } catch { return null; }
  if (!f || typeof f !== 'object') return null;
  const frame = f as Record<string, unknown>;
  if (frame.event === 'ack') return null;
  if (frame.event === 'fleet') { onFleetState?.(frame.fleet); return null; }
  if (frame.event === 'hello' || frame.event === 'snapshot') {
    const s = frame.snapshot ?? frame.payload;
    return isSnapshot(s) ? s as Snapshot : null;
  }
  return isSnapshot(f) ? f as Snapshot : null;
}

/** Connect to a loopback fleet relay. Returns an idempotent disposer. */
export function connectWs({ url, token: _token, onFleet, onStatus, onFleetState }: WsOptions): () => void {
  let ws: WebSocket | null = null;
  let retry = 0;
  let disposed = false;
  let timer: number | undefined;

  const setStatus = (s: FleetStatus) => { try { onStatus?.(s); } catch { /* never throw into render */ } };

  const open = () => {
    if (disposed) return;
    setStatus(retry === 0 ? 'connecting' : 'reconnecting');
    let socket: WebSocket;
    try { socket = new WebSocket(url); } catch {
      scheduleRetry(); return;
    }
    ws = socket;
    socket.onopen = () => { retry = 0; setStatus('live'); };
    socket.onmessage = (e) => {
      if (disposed) return;
      try {
        const snap = normalizeWsFrame(typeof e.data === 'string' ? e.data : '', onFleetState);
        if (snap) onFleet(snap);
      } catch { /* a bad frame must never break the render path */ }
    };
    socket.onerror = () => { /* onclose drives the retry */ };
    socket.onclose = () => { if (ws === socket) { ws = null; scheduleRetry(); } };
  };

  const scheduleRetry = () => {
    if (disposed) return;
    setStatus('offline');
    retry += 1;
    const delay = Math.min(MAX, FIRST * 2 ** Math.min(retry - 1, 4));
    timer = window.setTimeout(open, delay);
  };

  const FIRST = 500, MAX = 5000;
  open();
  return () => {
    disposed = true;
    if (timer !== undefined) window.clearTimeout(timer);
    if (ws) { try { ws.close(); } catch { /* already closed */ } ws = null; }
    setStatus('offline');
  };
}
