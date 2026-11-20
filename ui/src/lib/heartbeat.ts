// heartbeat.ts — DESIGN.md §9.2. Dot-matrix events/s field; React owns mount/unmount via useEffect.
import { Spring, ticker } from './motion';
import { SPRING } from './tokens';

export class Heartbeat {
  private ctx: CanvasRenderingContext2D; private cols: number; private rows: number;
  private vals: number[]; private off = new Spring(0, SPRING.open); private head = new Spring(0, SPRING.hover);
  constructor(private cv: HTMLCanvasElement, private pitch = 11, rows = 9) {
    const w = cv.clientWidth, h = rows * pitch, dpr = Math.min(window.devicePixelRatio || 1, 2);
    cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); cv.style.height = h + 'px';
    this.ctx = cv.getContext('2d')!; this.ctx.scale(dpr, dpr);
    this.cols = Math.floor(w / pitch); this.rows = rows; this.vals = new Array(this.cols).fill(0);
  }
  /** v: 0..1 */
  push(v: number) {
    this.vals.shift(); this.vals.push(Math.round(v * this.rows));
    this.off.x = 1; this.off.v = 0; this.off.target = 0; // slide one column left
    this.head.x = 0; this.head.v = 0; this.head.target = this.vals[this.cols - 1] ?? 0;
    ticker.add(this.draw);
  }
  drawOnce() { this.draw(0); }
  private draw = (dt: number) => {
    const { ctx, pitch, rows, cols } = this, o = this.off.step(dt), hd = this.head.step(dt);
    ctx.clearRect(0, 0, cols * pitch, rows * pitch);
    for (let c = 0; c < cols; c++) {
      const x = (c + o + 0.5) * pitch, fade = Math.min(1, (c + o) / 6);
      const lit = c === cols - 1 ? hd : (this.vals[c] ?? 0);
      for (let r = 0; r < rows; r++) {
        const on = Math.max(0, Math.min(1, lit - r)); // fractional last dot = partial alpha
        ctx.fillStyle = `rgba(245,208,254,${((0.18 + 0.78 * on) * fade).toFixed(3)})`;
        ctx.fillRect(Math.round(x) - 1.5, Math.round((rows - 1 - r + 0.5) * pitch) - 1.5, 3, 3);
      }
      if (c === cols - 1 && lit > 0) { // head pixel
        ctx.fillStyle = '#D946EF';
        ctx.fillRect(Math.round(x) - 2, Math.round((rows - 1 - lit + 0.5) * pitch) - pitch - 2, 4, 4);
      }
    }
    return !(this.off.settled && this.head.settled);
  };
}
