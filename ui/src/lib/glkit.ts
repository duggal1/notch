// glkit.ts — DESIGN.md §7.1. One fullscreen-triangle pass helper (orb + wave).
export type Uniforms = Record<string, number | number[]>;

const compile = (gl: WebGL2RenderingContext, type: number, src: string) => {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src); gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader');
  return s;
};

const FULLSCREEN_VS = `#version 300 es
void main(){ vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)); gl_Position = vec4(p * 2. - 1., 0., 1.); }`;

export class Pass {
  gl: WebGL2RenderingContext; dpr = 1;
  private prog: WebGLProgram; private locs = new Map<string, WebGLUniformLocation | null>();

  constructor(public canvas: HTMLCanvasElement, frag: string) {
    const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: true, antialias: false, powerPreference: 'low-power' });
    if (!gl) throw new Error('webgl2 unavailable');
    this.gl = gl;
    const prog = gl.createProgram()!;
    gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, FULLSCREEN_VS));
    gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, frag.trimStart())); // "#version" must be byte 0
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? 'link');
    this.prog = prog;
  }

  resize(cssW: number, cssH: number, cap = 2) {
    this.dpr = Math.min(window.devicePixelRatio || 1, cap);
    this.canvas.width = Math.round(cssW * this.dpr);
    this.canvas.height = Math.round(cssH * this.dpr);
  }

  draw(u: Uniforms) {
    const { gl } = this;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.useProgram(this.prog);
    gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT);
    for (const [k, v] of Object.entries(u)) {
      let l = this.locs.get(k);
      if (l === undefined) { l = gl.getUniformLocation(this.prog, k); this.locs.set(k, l); }
      if (!l) continue;
      if (typeof v === 'number') gl.uniform1f(l, v);
      else if (v.length === 2) gl.uniform2fv(l, v);
      else if (v.length === 4) gl.uniform4fv(l, v);
      else gl.uniform3fv(l, v); // vec3 and vec3[] (flat)
    }
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
