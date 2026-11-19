// testkit.ts — pure surface re-exported for test.mjs (no DOM at module scope).
export { frameFor, notchPath, type Mode, type Notch } from './lib/shell-geometry';
export { Spring, mulberry32, damp } from './lib/motion';
export { SPRING } from './lib/tokens';
export { computeFloatingIds, type FidInput, type FidPlacement } from './lib/floating-ids';
export { topStatus, workingCount, normalizeEps, fmtElapsed, drift, failureLabel, pad2 } from './lib/format';
export type { Session, Snapshot } from './lib/bridge';
