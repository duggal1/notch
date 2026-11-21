// ws-transport.ts — optional loopback WebSocket transport for the live fleet stream.
// Frames are NDJSON-shaped JSON (one object per text frame), exactly like the daemon's
// WebSocket wire: `hello` (full §1.3 snapshot), `snapshot` (full snapshot) and the additive
// `fleet` frame (`{event:'fleet', fleet:{units:[…]}}` or the snapshot-shaped variant).
//
// Browsers cannot set `Authorization`/`Host` and always send `Origin`, so a strict daemon
// handshake is not reachable from a webview. This transport therefore carries the token as
// a query parameter for a browser/demo relay; the Tauri app keeps `listen('snapshot')` as
// primary and only adds WS when a credential is explicitly supplied by the bridge.
// Nothing here touches the DOM or React state — it only emits normalized snapshots.
import type { Snapshot, Session, Supervisor } from './bridge';
import type { Status } from './tokens';

export const FLEET_EVENT = 'fleet' as const;

export type FleetStatus = 'connecting' | 'live' | 'reconnecting' | 'offline';

export interface WsCredentials {
  port: number;
  token?: string;
}

export interface WsOptions {
  url: string;
  token?: string;
  onFleet: (snapshot: Snapshot) => void;
  onStatus?: (status: FleetStatus) => void;
}

/** Read optional browser/demo credentials: `?token=…&wsPort=…` (default 7407). */
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

const STATUSES = new Set(['working', 'idle', 'failed', 'done']);
const normStatus = (v: unknown): Status | null =>
  typeof v === 'string' && STATUSES.has(v) ? v as Status
  : v === 'blocked' || v === 'stuck' ? 'failed'
  : v === 'unknown' ? 'idle' : null;

const isSnapshot = (v: unknown): v is Snapshot =>
  !!v && typeof v === 'object'
  && Array.isArray((v as Snapshot).sessions)
  && Array.isArray((v as Snapshot).supervisors);

function emptySnapshot(): Snapshot {
  return {
    version: 1,
    generatedAt: Date.now(),
    orchestrator: { id: 'ORCH', status: 'idle' },
    supervisors: [],
    sessions: [],
    eps: 0,
    conflicts: [],
    attribution: [],
    warnings: [],
  };
}

interface FleetUnit {
  id?: unknown;
  kind?: unknown;
  sup?: unknown;
  supervisor?: unknown;
  status?: unknown;
  title?: unknown;
  task?: unknown;
  cwd?: unknown;
  workspace?: unknown;
  paneId?: unknown;
  startedAt?: unknown;
  lastAction?: unknown;
  revision?: unknown;
  lastRevision?: unknown;
  latencyMs?: unknown;
  failureReason?: unknown;
  working?: unknown;
  idle?: unknown;
  failed?: unknown;
  done?: unknown;
}

/**
 * Adapt an additive `fleet` payload into the existing §1.3 Snapshot shape.
 * Known fields are patched onto the last snapshot; everything else (titles, cwd,
 * DoD, decisions, bus messages, layout-relevant data) is preserved verbatim.
 */
export function adaptFleet(fleet: unknown, baseline: Snapshot | null): Snapshot | null {
  if (!fleet || typeof fleet !== 'object') return baseline;
  const f = fleet as Record<string, unknown>;

  // Snapshot-shaped fleet: supervisors/sessions arrays already present.
  if (Array.isArray(f.sessions) && Array.isArray(f.supervisors)) {
    const merged = { ...(baseline ?? emptySnapshot()), ...f } as Snapshot;
    return isSnapshot(merged) ? merged : baseline;
  }

  const units = Array.isArray(f.units) ? f.units as FleetUnit[] : null;
  if (!units) return baseline;

  const snap: Snapshot = baseline
    ? { ...baseline, supervisors: baseline.supervisors.map((s) => ({ ...s })), sessions: baseline.sessions.map((s) => ({ ...s })) }
    : emptySnapshot();

  const fallbackSup = snap.supervisors[0]?.id ?? 'SV-1';
  for (const u of units) {
    if (!u || typeof u.id !== 'string') continue;
    const id = u.id;
    const kind = typeof u.kind === 'string' ? u.kind
      : id === 'ORCH' ? 'orchestrator' : id.startsWith('SV-') ? 'supervisor' : 'session';
    const status = normStatus(u.status);

    if (kind === 'orchestrator' && snap.orchestrator.id === id) {
      if (status) snap.orchestrator = { ...snap.orchestrator, status };
      continue;
    }
    if (kind === 'supervisor') {
      const sv: Supervisor | undefined = snap.supervisors.find((s) => s.id === id);
      if (sv) {
        if (status) sv.status = status;
        for (const k of ['working', 'idle', 'failed', 'done'] as const)
          if (typeof u[k] === 'number') sv[k] = u[k] as number;
      } else {
        snap.supervisors.push({
          id, status: status ?? 'idle',
          working: typeof u.working === 'number' ? u.working : 0,
          idle: typeof u.idle === 'number' ? u.idle : 0,
          failed: typeof u.failed === 'number' ? u.failed : 0,
          done: typeof u.done === 'number' ? u.done : 0,
        });
      }
      continue;
    }
    if (kind !== 'session') continue;

    const existing: Session | undefined = snap.sessions.find((s) => s.id === id);
    if (existing) {
      if (status) existing.status = status;
      if (typeof u.lastAction === 'string') existing.lastAction = u.lastAction;
      const rev = typeof u.revision === 'number' ? u.revision : u.lastRevision;
      if (typeof rev === 'number') existing.lastRevision = rev;
      if (typeof u.paneId === 'string' && u.paneId) existing.paneId = u.paneId;
      if (typeof u.title === 'string' && u.title) existing.title = u.title;
      continue;
    }
    const sup = typeof u.sup === 'string' ? u.sup
      : typeof u.supervisor === 'string' ? u.supervisor : fallbackSup;
    snap.sessions.push({
      id, sup, status: status ?? 'idle',
      title: typeof u.title === 'string' && u.title ? u.title : id,
      task: typeof u.task === 'string' ? u.task : (typeof u.title === 'string' ? u.title : id),
      cwd: typeof u.cwd === 'string' ? u.cwd : (typeof u.workspace === 'string' ? u.workspace : ''),
      paneId: typeof u.paneId === 'string' ? u.paneId : '',
      startedAt: typeof u.startedAt === 'number' ? u.startedAt : Date.now(),
      lastAction: typeof u.lastAction === 'string' ? u.lastAction : '',
      lastRevision: typeof u.revision === 'number' ? u.revision
        : typeof u.lastRevision === 'number' ? u.lastRevision : 0,
      latencyMs: typeof u.latencyMs === 'number' ? u.latencyMs : null,
      failureReason: typeof u.failureReason === 'string' ? u.failureReason : null,
    });
  }

  const top = normStatus(f.topStatus);
  if (top) snap.orchestrator = { ...snap.orchestrator, status: top };
  snap.generatedAt = typeof f.generatedAt === 'number' ? f.generatedAt : Date.now();
  return snap;
}

/** Parse one WS text frame into a Snapshot, or null for ack/event frames. */
export function normalizeWsFrame(raw: string, baseline: Snapshot | null): Snapshot | null {
  let f: unknown;
  try { f = JSON.parse(raw); } catch { return null; }
  if (!f || typeof f !== 'object') return null;
  const frame = f as Record<string, unknown>;
  if (frame.event === 'ack') return null;
  if (frame.event === 'fleet') return adaptFleet(frame.fleet, baseline);
  if (frame.event === 'hello' || frame.event === 'snapshot') {
    const s = frame.snapshot ?? frame.fleet ?? frame.payload;
    return isSnapshot(s) ? s as Snapshot : adaptFleet(s, baseline);
  }
  return isSnapshot(f) ? f as Snapshot : null;
}

/** Connect to a loopback fleet relay. Returns an idempotent disposer. */
export function connectWs({ url, token: _token, onFleet, onStatus }: WsOptions): () => void {
  let ws: WebSocket | null = null;
  let retry = 0;
  let disposed = false;
  let timer: number | undefined;
  let baseline: Snapshot | null = null;
  const FIRST = 500, MAX = 5000;

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
        const snap = normalizeWsFrame(typeof e.data === 'string' ? e.data : '', baseline);
        if (snap) { baseline = snap; onFleet(snap); }
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

  open();
  return () => {
    disposed = true;
    if (timer !== undefined) window.clearTimeout(timer);
    if (ws) { try { ws.close(); } catch { /* already closed */ } ws = null; }
    setStatus('offline');
  };
}
