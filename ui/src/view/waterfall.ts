// SPDX-License-Identifier: GPL-3.0-or-later
// WebGL2 waterfall. History lines live in a ring texture (one row per
// line, newest at the top of the screen). Each line keeps its own
// frequency mapping (first bin, bin width) in a float texture, and the
// shader maps every pixel's frequency through it, so old lines stay where
// they were when the view pans or zooms instead of smearing.
//
// Float32 has 24 mantissa bits (~4 Hz at 60 MHz), so absolute frequencies
// are split into a block number (65536 Hz blocks) and a remainder. The
// shader subtracts the block numbers as integers: float expressions that
// rely on exact cancellation (a - b with a == b, both ~6e6) are not safe,
// as compilers may reassociate them (seen on an NVIDIA driver: the whole
// waterfall mapped below the line's first bin).

import type { WfLine } from "../proto.ts";
import { colormap } from "./colormap.ts";

const ROWS = 2048;
const MAX_BINS = 4096;
const SPLIT = 65536;

const VS = `#version 300 es
in vec2 p;
void main() { gl_Position = vec4(p, 0.0, 1.0); }`;

const FS = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D uHist;   // R8, MAX_BINS x ROWS
uniform sampler2D uParams; // RGBA32F, 2 x ROWS
uniform sampler2D uLut;    // RGBA8, 256 x 1
uniform int uViewBlk;
uniform float uViewLo, uSpan, uWidth, uHeight, uMin, uMax;
uniform int uHead, uCount, uDebug;
out vec4 color;
// no data: distinct from the palette's lowest colour
const vec4 BG = vec4(0.06, 0.03, 0.06, 1.0);
void main() {
  int row = int(uHeight - gl_FragCoord.y);
  if (row >= uCount) { color = uDebug == 1 ? vec4(1, 0, 0, 1) : BG; return; }
  int line = (uHead - row + ${ROWS}) % ${ROWS};
  vec4 p0 = texelFetch(uParams, ivec2(0, line), 0); // f0 block, f0 lo, df, bins
  vec4 p1 = texelFetch(uParams, ivec2(1, line), 0); // db0, step
  float off = float(uViewBlk - int(p0.x)) * ${SPLIT}.0 + (uViewLo - p0.y) + gl_FragCoord.x / uWidth * uSpan;
  float b = floor(off / p0.z + 0.5);
  if (uDebug == 1 && p0.z <= 0.0) { color = vec4(1, 1, 0, 1); return; }
  if (b < 0.0 || b >= p0.w) { color = uDebug == 1 ? (b < 0.0 ? vec4(0, 1, 0, 1) : vec4(0, 0, 1, 1)) : BG; return; }
  float v = texelFetch(uHist, ivec2(int(b), line), 0).r * 255.0;
  float db = p1.x + v * p1.y;
  float t = clamp((db - uMin) / (uMax - uMin), 0.0, 1.0);
  color = texture(uLut, vec2(t, 0.5));
}`;

/** Block number (65536 Hz) and the remainder. */
function split(hz: number): [number, number] {
  const blk = Math.floor(hz / SPLIT);
  return [blk, hz - blk * SPLIT];
}

export class Waterfall {
  private canvas: HTMLCanvasElement;
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private hist: WebGLTexture;
  private params: WebGLTexture;
  private u: Record<string, WebGLUniformLocation | null> = {};
  private head = -1;
  private count = 0;
  private tested = false;
  private debug = new URLSearchParams(location.search).has("wfdebug");
  private lastLog = 0;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    const gl = canvas.getContext("webgl2", { antialias: false, preserveDrawingBuffer: false });
    if (!gl) throw new Error("WebGL2 is not available");
    this.gl = gl;
    this.prog = link(gl, VS, FS);
    gl.useProgram(this.prog);
    for (const n of ["uHist", "uParams", "uLut", "uViewBlk", "uViewLo", "uSpan", "uWidth", "uHeight", "uMin", "uMax", "uHead", "uCount", "uDebug"])
      this.u[n] = gl.getUniformLocation(this.prog, n);

    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(this.prog, "p");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    this.hist = texture(gl, 0, gl.R8, MAX_BINS, ROWS, gl.RED, gl.UNSIGNED_BYTE);
    this.params = texture(gl, 1, gl.RGBA32F, 2, ROWS, gl.RGBA, gl.FLOAT);
    const lut = texture(gl, 2, gl.RGBA8, 256, 1, gl.RGBA, gl.UNSIGNED_BYTE, colormap());
    gl.bindTexture(gl.TEXTURE_2D, lut);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.uniform1i(this.u.uHist, 0);
    gl.uniform1i(this.u.uParams, 1);
    gl.uniform1i(this.u.uLut, 2);
  }

  push(l: WfLine): void {
    const gl = this.gl;
    this.head = (this.head + 1) % ROWS;
    this.count = Math.min(this.count + 1, ROWS);
    const n = Math.min(l.levels.length, MAX_BINS);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.hist);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, this.head, n, 1, gl.RED, gl.UNSIGNED_BYTE, l.levels.subarray(0, n));
    const [blk, lo] = split(l.f0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.params);
    const params = new Float32Array([blk, lo, l.df, n, l.db0, l.dbStep, 0, 0]);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, this.head, 2, 1, gl.RGBA, gl.FLOAT, params);
    if (!this.tested) {
      this.tested = true;
      this.selfTest(l.levels.subarray(0, 8), params);
    }
  }

  /** Read the first line back from both textures and log what came back
   * (drivers differ in what they accept; a silent upload failure shows
   * as an empty waterfall). */
  private selfTest(levels: Uint8Array, params: Float32Array): void {
    const gl = this.gl;
    const info = gl.getExtension("WEBGL_debug_renderer_info");
    const renderer = info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
    const err = gl.getError();
    const read = (tex: WebGLTexture, w: number, type: number, out: ArrayBufferView): string => {
      const fb = gl.createFramebuffer();
      gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
      const st = gl.checkFramebufferStatus(gl.FRAMEBUFFER);
      let r = `fb ${st === gl.FRAMEBUFFER_COMPLETE ? "ok" : "0x" + st.toString(16)}`;
      if (st === gl.FRAMEBUFFER_COMPLETE) {
        gl.readPixels(0, this.head, w, 1, gl.RGBA, type, out);
        r += ` err ${gl.getError()}`;
      }
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.deleteFramebuffer(fb);
      return r;
    };
    const h = new Uint8Array(8 * 4);
    const hs = read(this.hist, 8, gl.UNSIGNED_BYTE, h);
    const cbf = gl.getExtension("EXT_color_buffer_float");
    const p = new Float32Array(8);
    const ps = cbf ? read(this.params, 2, gl.FLOAT, p) : "no EXT_color_buffer_float";
    console.info(
      "waterfall self-test:",
      JSON.stringify({
        renderer,
        uploadError: err,
        levelsSent: [...levels],
        levelsRead: [...h].filter((_, i) => i % 4 === 0),
        levelsReadStatus: hs,
        paramsSent: [...params],
        paramsRead: [...p],
        paramsReadStatus: ps,
      }),
    );
  }

  /** Draw the history for the frequency view [start, start + span). */
  draw(start: number, span: number, minDb: number, maxDb: number): void {
    const gl = this.gl;
    const c = this.canvas;
    gl.viewport(0, 0, c.width, c.height);
    gl.useProgram(this.prog);
    const [blk, lo] = split(start);
    gl.uniform1i(this.u.uViewBlk, blk);
    gl.uniform1f(this.u.uViewLo, lo);
    gl.uniform1f(this.u.uSpan, span);
    gl.uniform1f(this.u.uWidth, c.width);
    gl.uniform1f(this.u.uHeight, c.height);
    gl.uniform1f(this.u.uMin, minDb);
    gl.uniform1f(this.u.uMax, Math.max(maxDb, minDb + 1));
    gl.uniform1i(this.u.uHead, Math.max(this.head, 0));
    gl.uniform1i(this.u.uCount, Math.min(this.count, c.height));
    gl.uniform1i(this.u.uDebug, this.debug ? 1 : 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    if (this.debug && performance.now() - this.lastLog > 1000) {
      this.lastLog = performance.now();
      console.info(
        "waterfall draw:",
        JSON.stringify({
          start,
          span,
          blk,
          lo,
          minDb,
          maxDb,
          head: this.head,
          count: this.count,
          w: c.width,
          h: c.height,
          err: gl.getError(),
          // what the GPU actually holds
          gpu: Object.fromEntries(
            ["uViewBlk", "uViewLo", "uSpan", "uWidth", "uHeight", "uHead", "uCount"].map((n) => [
              n,
              this.u[n] ? gl.getUniform(this.prog, this.u[n]!) : "no location",
            ]),
          ),
        }),
      );
    }
  }
}

function texture(
  gl: WebGL2RenderingContext,
  unit: number,
  internal: number,
  w: number,
  h: number,
  format: number,
  type: number,
  data: ArrayBufferView | null = null,
): WebGLTexture {
  const t = gl.createTexture()!;
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, t);
  gl.texImage2D(gl.TEXTURE_2D, 0, internal, w, h, 0, format, type, data);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return t;
}

function link(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const sh = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
    return s;
  };
  const p = gl.createProgram()!;
  gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
  gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "link");
  return p;
}
