// The browser frontend's script: paint the display protocol (ADR-046) on a canvas and
// post the tab's input back through the editor/web bridge. One EventSource in, one POST
// per input message out. Everything positional is in the canvas's backing pixels
// (device pixels), which is what a Brood window reports too.

"use strict";

const canvas = document.getElementById("c");
const ctx = canvas.getContext("2d");
const status = document.getElementById("status");

// --- state -------------------------------------------------------------------------------

let tabId = null;
let lastFrame = [];
const textures = new Map(); // handle -> { bitmap, w, h }
let pixelInput = false;      // the app asked for pixels (else cells) in mouse messages
const sounds = new Map();   // handle -> AudioBuffer
let audio = null;           // the AudioContext, created on the first user gesture

// The cell grid: a monospace font sized in device pixels, its cell measured once.
const FONT_PX = 15;
let cell = { w: 9, h: 21, baseline: 15 };

function measureCell() {
  ctx.font = `${FONT_PX * devicePixelRatio}px monospace`;
  const m = ctx.measureText("M");
  const ascent = m.actualBoundingBoxAscent || FONT_PX * devicePixelRatio * 0.8;
  const descent = m.actualBoundingBoxDescent || FONT_PX * devicePixelRatio * 0.2;
  cell = {
    w: Math.ceil(m.width),
    h: Math.ceil((ascent + descent) * 1.4),
    baseline: Math.round(ascent + (ascent + descent) * 0.2),
  };
}

// --- colours ---------------------------------------------------------------------------------
// The same palette the runtime's `color_rgb` maps face keywords to.

const PALETTE = {
  black: [0, 0, 0], red: [0xcd, 0x31, 0x31], green: [0x0d, 0xbc, 0x79],
  yellow: [0xe5, 0xe5, 0x10], blue: [0x24, 0x72, 0xc8], magenta: [0xbc, 0x3f, 0xbc],
  cyan: [0x11, 0xa8, 0xcd], white: [0xe5, 0xe5, 0xe5], grey: [0x80, 0x80, 0x80],
  gray: [0x80, 0x80, 0x80], "dark-grey": [0x50, 0x50, 0x50], "dark-gray": [0x50, 0x50, 0x50],
};
const DEFAULT_FG = "rgb(229,229,229)";
const BACKGROUND = "rgb(12,12,16)";

// A colour value (keyword string, [r g b], [r g b a], "#rgb"/"#rrggbb") as a CSS colour,
// or null for "none".
function css(color) {
  if (color == null) return null;
  if (Array.isArray(color)) {
    if (color.length === 3) return `rgb(${color[0]},${color[1]},${color[2]})`;
    if (color.length === 4) return `rgba(${color[0]},${color[1]},${color[2]},${color[3] / 255})`;
    return null;
  }
  if (typeof color === "string") {
    if (color.startsWith("#")) return color;
    const p = PALETTE[color];
    return p ? `rgb(${p[0]},${p[1]},${p[2]})` : null;
  }
  return null;
}

// --- effects (ADR-413): lighting, bloom, CRT, particles ---------------------------------------
// The runtime's arithmetic where a 2D canvas can follow it: the same light falloff, the same
// particle bookkeeping and random numbers (brood's host/gui/effects.rs), so a tab sees what a
// window sees. Two approximations: a particle is a flat disc rather than a soft one, and the
// CRT has scanlines and a vignette but no curvature.

const MAX_PARTICLES = 65536;
const MAX_STEP_SECONDS = 0.1;

// lowbias32, and a uniform [0, 1) per slot and draw: effects.rs `hash` / `rand`.
function hash(x) {
  let v = x >>> 0;
  v ^= v >>> 16; v = Math.imul(v, 0x7feb352d) >>> 0;
  v ^= v >>> 15; v = Math.imul(v, 0x846ca68b) >>> 0;
  v ^= v >>> 16;
  return v >>> 0;
}
function rand(seed, slot, k) {
  return (hash((seed ^ hash((Math.imul(slot, 4) + k) >>> 0)) >>> 0) >>> 8) / 16777216;
}

// An emitter op's options with the runtime's defaults (`EmitterSpec::default`).
function emitterSpec(opts) {
  const o = opts && typeof opts === "object" && !Array.isArray(opts) ? opts : {};
  const pair = (v, d) => Array.isArray(v) && v.length === 2 ? v : typeof v === "number" ? [v, v] : d;
  const rgba = (v, d) => {
    if (Array.isArray(v) && v.length === 4) return v;
    if (Array.isArray(v) && v.length === 3) return [...v, 255];
    if (typeof v === "string" && PALETTE[v]) return [...PALETTE[v], 255];
    return d;
  };
  const color = rgba(o.color, [255, 255, 255, 255]);
  return {
    rate: Math.max(0, typeof o.rate === "number" ? o.rate : 0),
    life: pair(o.life, [1, 1]).map((x) => Math.max(0.001, x)),
    speed: pair(o.speed, [50, 100]),
    angle: typeof o.angle === "number" ? o.angle : -Math.PI / 2,
    spread: typeof o.spread === "number" ? Math.max(0, Math.min(2 * Math.PI, o.spread)) : 2 * Math.PI,
    gravity: pair(o.gravity, [0, 0]),
    drag: Math.max(0, typeof o.drag === "number" ? o.drag : 0),
    size: pair(o.size, [4, 4]).map((x) => Math.max(0, x)),
    color,
    colorEnd: rgba(o["color-end"], [color[0], color[1], color[2], 0]),
    additive: o.blend === "add",
    square: o.shape === "square",
    max: Math.max(1, Math.min(MAX_PARTICLES, Number.isInteger(o.max) ? o.max : 1024)),
  };
}

// Every op of a frame, regions included.
function walkOps(ops, f) {
  for (const op of ops) {
    if (!Array.isArray(op)) continue;
    if (op[0] === "scroll-region") walkOps(op[2], f);
    else if (op[0] === "cell-region") walkOps(op[6], f);
    else if (op[0] === "above-seam") walkOps(op[1], f);
    else f(op);
  }
}

// effects.rs `EmitterBook` + `CpuParticles`: the tab's particle systems.
const emitters = new Map(); // id -> { x, y, spec, cursor, owed, sinceSpawn, steps, present, ring }
let pendingBursts = new Map();
let lastStep = null;

function burstsArrived(frame) {
  walkOps(frame, (op) => {
    if (op[0] === "burst" && Number.isInteger(op[1]) && Number.isInteger(op[2]) && op[2] > 0) {
      pendingBursts.set(op[1], Math.min(MAX_PARTICLES, (pendingBursts.get(op[1]) || 0) + op[2]));
    }
  });
}

function stepParticles(frame) {
  const now = performance.now() / 1000;
  const dt = lastStep == null ? 0 : Math.min(MAX_STEP_SECONDS, Math.max(0, now - lastStep));
  lastStep = now;
  for (const e of emitters.values()) e.present = false;
  walkOps(frame, (op) => {
    if (op[0] !== "emitter" || !Number.isInteger(op[1]) || op[1] < 0) return;
    const spec = emitterSpec(op[4]);
    let e = emitters.get(op[1]);
    if (!e || e.spec.max !== spec.max) {
      e = { cursor: 0, owed: 0, sinceSpawn: Infinity, steps: 0, ring: new Float32Array(spec.max * 6) };
      emitters.set(op[1], e);
    }
    Object.assign(e, { x: op[2], y: op[3], spec, present: true });
  });
  const bursts = pendingBursts;
  pendingBursts = new Map();
  for (const [id, e] of emitters) {
    const s = e.spec, capacity = s.max;
    let spawn = 0;
    if (e.present) {
      const due = e.owed + s.rate * dt;
      const whole = Math.min(Math.floor(due), capacity);
      e.owed = Math.max(0, due - whole);
      spawn = whole + (bursts.get(id) || 0);
    }
    spawn = Math.min(spawn, capacity);
    e.sinceSpawn = spawn > 0 ? 0 : e.sinceSpawn + dt;
    if (!e.present && e.sinceSpawn > Math.max(s.life[0], s.life[1])) { emitters.delete(id); continue; }
    const seed = hash((Math.imul(id, 0x9e3779b9) ^ e.steps) >>> 0);
    const first = e.cursor % capacity;
    // slot: x y vx vy age life
    for (let n = 0; n < spawn; n++) {
      const slot = (first + n) % capacity, p = slot * 6;
      const angle = s.angle + (rand(seed, slot, 0) - 0.5) * s.spread;
      const speed = s.speed[0] + (s.speed[1] - s.speed[0]) * rand(seed, slot, 1);
      const life = s.life[0] + (s.life[1] - s.life[0]) * rand(seed, slot, 2);
      e.ring.set([e.x, e.y, Math.cos(angle) * speed, Math.sin(angle) * speed, 0, Math.max(life, 0.001)], p);
    }
    const keep = Math.max(0, 1 - s.drag * dt);
    for (let p = 0; p < e.ring.length; p += 6) {
      const r = e.ring;
      if (r[p + 5] <= 0 || r[p + 4] >= r[p + 5]) continue;
      r[p + 2] = (r[p + 2] + s.gravity[0] * dt) * keep;
      r[p + 3] = (r[p + 3] + s.gravity[1] * dt) * keep;
      r[p] += r[p + 2] * dt;
      r[p + 1] += r[p + 3] * dt;
      r[p + 4] += dt;
    }
    e.cursor = (first + spawn) % capacity;
    e.steps = (e.steps + 1) >>> 0;
  }
}

function particlesAlive() {
  for (const e of emitters.values()) {
    if (e.present || e.sinceSpawn <= Math.max(e.spec.life[0], e.spec.life[1])) return true;
  }
  return false;
}

function drawParticles(id) {
  const e = emitters.get(id);
  if (!e) return;
  const s = e.spec, r = e.ring;
  ctx.save();
  ctx.globalCompositeOperation = s.additive ? "lighter" : "source-over";
  for (let p = 0; p < r.length; p += 6) {
    if (r[p + 5] <= 0 || r[p + 4] >= r[p + 5]) continue;
    const t = Math.min(1, r[p + 4] / r[p + 5]);
    const size = s.size[0] + (s.size[1] - s.size[0]) * t;
    const c = (i) => Math.round(s.color[i] + (s.colorEnd[i] - s.color[i]) * t);
    // the soft disc as a flat one: half its width, at 0.6 of its alpha
    ctx.globalAlpha = (c(3) / 255) * (s.square ? 1 : 0.6);
    ctx.fillStyle = `rgb(${c(0)},${c(1)},${c(2)})`;
    if (s.square) ctx.fillRect(r[p] - size / 2, r[p + 1] - size / 2, size, size);
    else {
      ctx.beginPath();
      ctx.arc(r[p], r[p + 1], size / 4, 0, 2 * Math.PI);
      ctx.fill();
    }
  }
  ctx.restore();
}

// effects.rs `FrameEffects::of`.
function frameEffects(frame) {
  const fx = { ambient: null, lights: [], overlayAt: frame.findIndex((op) => Array.isArray(op) && op[0] === "overlay"), bloom: null, crt: null };
  const num = (v, d) => typeof v === "number" && Number.isFinite(v) ? v : d;
  const rgb = (v) => {
    if (Array.isArray(v) && v.length >= 3) return v.slice(0, 3);
    if (typeof v === "string" && PALETTE[v]) return PALETTE[v];
    return null;
  };
  walkOps(frame, (op) => {
    switch (op[0]) {
      case "ambient": if (!fx.ambient) fx.ambient = rgb(op[1]); break;
      case "light": {
        const color = rgb(op[4]), radius = num(op[3], 0);
        if (color && radius > 0) fx.lights.push([num(op[1], 0), num(op[2], 0), radius, color, Math.max(0, num(op[5], 1))]);
        break;
      }
      case "bloom": if (!fx.bloom) fx.bloom = [num(op[1], 0.7), num(op[2], 0.8), num(op[3], 8)]; break;
      case "crt": if (!fx.crt) fx.crt = [num(op[1], 0.25), num(op[2], 0.15), num(op[3], 0.3)]; break;
    }
  });
  return fx;
}

const lightCanvas = document.createElement("canvas");
const lightCtx = lightCanvas.getContext("2d");

// The light map — the ambient, plus each light's (1 - d²/r²)² in gradient stops — multiplied
// onto what is drawn so far.
function applyLighting(fx) {
  lightCanvas.width = canvas.width;
  lightCanvas.height = canvas.height;
  const [ar, ag, ab] = fx.ambient;
  lightCtx.globalCompositeOperation = "source-over";
  lightCtx.fillStyle = `rgb(${ar},${ag},${ab})`;
  lightCtx.fillRect(0, 0, canvas.width, canvas.height);
  lightCtx.globalCompositeOperation = "lighter";
  for (const [x, y, radius, [r, g, b], intensity] of fx.lights) {
    const gradient = lightCtx.createRadialGradient(x, y, 0, x, y, radius);
    for (let i = 0; i <= 8; i++) {
      const f = i / 8, k = (1 - f * f) ** 2 * intensity;
      gradient.addColorStop(f, `rgb(${Math.min(255, r * k)},${Math.min(255, g * k)},${Math.min(255, b * k)})`);
    }
    lightCtx.fillStyle = gradient;
    lightCtx.fillRect(x - radius, y - radius, 2 * radius, 2 * radius);
  }
  ctx.save();
  ctx.globalCompositeOperation = "multiply";
  ctx.drawImage(lightCanvas, 0, 0);
  ctx.restore();
}

const bloomCanvas = document.createElement("canvas");
const bloomCtx = bloomCanvas.getContext("2d");
let bloomFilter = null;

// The bright pass as an SVG filter (the luma ramp from the threshold up), blurred by
// the canvas filter, then added back.
function applyBloom([threshold, intensity, radius]) {
  const slope = 1 / Math.max(1e-3, 1 - threshold);
  const id = `brood-bloom-${String(threshold).replace(".", "_")}`;
  if (!bloomFilter || bloomFilter.id !== id) {
    if (bloomFilter) bloomFilter.svg.remove();
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("width", "0");
    svg.setAttribute("height", "0");
    svg.style.position = "absolute";
    // the pixel's luma as alpha, ramped from the threshold, keeping that much of its colour
    svg.innerHTML = `<filter id="${id}" color-interpolation-filters="sRGB">
      <feColorMatrix in="SourceGraphic" type="luminanceToAlpha" result="luma"/>
      <feComponentTransfer in="luma" result="knee">
        <feFuncA type="linear" slope="${slope}" intercept="${-threshold * slope}"/>
      </feComponentTransfer>
      <feComposite in="SourceGraphic" in2="knee" operator="in"/>
    </filter>`;
    document.body.appendChild(svg);
    bloomFilter = { id, svg };
  }
  bloomCanvas.width = canvas.width;
  bloomCanvas.height = canvas.height;
  bloomCtx.filter = `url(#${id}) blur(${radius / 2}px)`;
  bloomCtx.drawImage(canvas, 0, 0);
  ctx.save();
  ctx.globalCompositeOperation = "lighter";
  ctx.globalAlpha = Math.min(1, intensity);
  ctx.drawImage(bloomCanvas, 0, 0);
  ctx.restore();
}

// Every other row dimmed, the corners darkened: effects.rs `crt_shade`, without the bulge.
function applyCrt([scanlines, , vignette]) {
  ctx.save();
  ctx.fillStyle = `rgba(0,0,0,${scanlines})`;
  for (let y = 1; y < canvas.height; y += 2) ctx.fillRect(0, y, canvas.width, 1);
  const cx = canvas.width / 2, cy = canvas.height / 2;
  const gradient = ctx.createRadialGradient(cx, cy, 0, cx, cy, Math.hypot(cx, cy));
  gradient.addColorStop(0, "rgba(0,0,0,0)");
  gradient.addColorStop(1, `rgba(0,0,0,${Math.min(1, vignette)})`);
  ctx.fillStyle = gradient;
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.restore();
}

// While particles fly the tab repaints on its own, as a window does.
let particleFrame = null;
function scheduleParticles() {
  if (particleFrame == null && particlesAlive()) {
    particleFrame = requestAnimationFrame(() => {
      particleFrame = null;
      paintFrame(lastFrame);
    });
  }
}

// --- painting --------------------------------------------------------------------------------

// Every op kind the protocol has. Cell ops land on the grid at (ox, oy) with a scroll
// shift `dy`; pixel ops (quad, sprite) are canvas pixels as given.
function paintOps(ops, ox, oy, cw, ch, dy) {
  for (const op of ops) {
    const tag = op[0];
    switch (tag) {
      case "clear":
        ctx.fillStyle = BACKGROUND;
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        break;
      case "text": {
        const [, row, col, s, face] = op;
        const f = face || {};
        const scale = Math.max(1, f.scale || 1);
        let fg = css(f.fg) || DEFAULT_FG;
        let bg = css(f.bg);
        if (f.reverse) { const t = fg; fg = bg || BACKGROUND; bg = t; }
        const top = oy + row * ch - dy;
        const chars = Array.from(s);
        let cx = col;
        ctx.font = `${f.bold ? "bold " : ""}${f.italic ? "italic " : ""}${FONT_PX * devicePixelRatio * scale}px monospace`;
        ctx.textBaseline = "alphabetic";
        for (const g of chars) {
          const cells = g === "\t" ? 8 - (cx % 8) : 1;
          const left = ox + cx * cw;
          if (bg) { ctx.fillStyle = bg; ctx.fillRect(left, top, cells * cw * scale, ch * scale); }
          if (g !== " " && g !== "\t") {
            ctx.fillStyle = fg;
            ctx.fillText(g, left, top + cell.baseline * scale);
            if (f.underline) ctx.fillRect(left, top + ch * scale - 2 * scale, cw * scale, scale);
          }
          cx += cells * scale;
        }
        break;
      }
      case "text-px": {
        // Pixel text: the text walk from a pixel top-left, shifted by the run's width
        // for :center / :right alignment; untouched by grid or scroll.
        const [, x, y, s, face] = op;
        const f = face || {};
        const scale = Math.max(1, f.scale || 1);
        let fg = css(f.fg) || DEFAULT_FG;
        let bg = css(f.bg);
        if (f.reverse) { const t = fg; fg = bg || BACKGROUND; bg = t; }
        const chars = Array.from(s);
        const width = chars.length * cw * scale;
        const left0 = f.align === "center" ? x - width / 2 : f.align === "right" ? x - width : x;
        ctx.font = `${f.bold ? "bold " : ""}${f.italic ? "italic " : ""}${FONT_PX * devicePixelRatio * scale}px monospace`;
        ctx.textBaseline = "alphabetic";
        chars.forEach((g, i) => {
          const left = left0 + i * cw * scale;
          if (bg) { ctx.fillStyle = bg; ctx.fillRect(left, y, cw * scale, ch * scale); }
          if (g !== " ") {
            ctx.fillStyle = fg;
            ctx.fillText(g, left, y + cell.baseline * scale);
            if (f.underline) ctx.fillRect(left, y + ch * scale - 2 * scale, cw * scale, scale);
          }
        });
        break;
      }
      case "rect": {
        const [, row, col, w, h, face, radius] = op;
        const f = face || {};
        const bg = css(f.reverse ? f.fg : f.bg);
        if (bg) roundRect(ox + col * cw, oy + row * ch - dy, w * cw, h * ch, (radius || 0) * cw, bg);
        break;
      }
      case "frect": {
        const [, x, y, w, h, face, opacity, radius] = op;
        const f = face || {};
        const bg = css(f.reverse ? f.fg : f.bg);
        if (bg) {
          ctx.globalAlpha = opacity == null ? 1 : opacity;
          roundRect(ox + x * cw, oy + y * ch - dy, w * cw, h * ch, (radius || 0) * cw, bg);
          ctx.globalAlpha = 1;
        }
        break;
      }
      case "cursor": {
        const [, row, col, style] = op;
        const left = ox + col * cw, top = oy + row * ch - dy;
        ctx.fillStyle = "rgb(245,245,245)";
        if (style === "bar") ctx.fillRect(left, top, Math.max(2, cw / 8), ch);
        else if (style === "underline") ctx.fillRect(left, top + ch - Math.max(2, ch / 10), cw, Math.max(2, ch / 10));
        else {
          ctx.fillStyle = "rgba(255,255,255,0.5)";
          ctx.fillRect(left, top, cw, ch);
          const t = Math.max(2, Math.floor(cw / 10));
          ctx.fillStyle = "rgb(245,245,245)";
          ctx.fillRect(left, top, cw, t); ctx.fillRect(left, top + ch - t, cw, t);
          ctx.fillRect(left, top, t, ch); ctx.fillRect(left + cw - t, top, t, ch);
        }
        break;
      }
      case "vspans": {
        const [, row0, col0, cols] = op;
        cols.forEach((segs, i) => {
          let y = oy + row0 * ch - dy;
          for (const [sh, color] of segs) {
            const c = css(color);
            if (c) { ctx.fillStyle = c; ctx.fillRect(ox + (col0 + i) * cw, y, cw, sh * ch); }
            y += sh * ch;
          }
        });
        break;
      }
      case "scroll-region": {
        const [, dyFrac, inner] = op;
        paintOps(inner, ox, oy, cw, ch, Math.round(dyFrac * ch));
        break;
      }
      case "above-seam":
        // no leftover strip here (the tab's grid has no seam): the ops paint where they are
        paintOps(op[1], ox, oy, cw, ch, dy);
        break;
      case "cell-region": {
        // The region's own font size: scale the grid by px / FONT_PX and clip its rows.
        const [, x, y, w, h, px, inner] = op;
        const k = px / FONT_PX;
        const top = oy + y * ch - dy;
        ctx.save();
        ctx.beginPath();
        ctx.rect(0, Math.max(0, top), canvas.width, h * ch);
        ctx.clip();
        paintOps(inner, ox + x * cw, Math.max(0, top), cw * k, ch * k, 0);
        ctx.restore();
        break;
      }
      case "quad": {
        const [, x, y, w, h, color, rot] = op;
        const c = css(color);
        if (!c) break;
        ctx.save();
        ctx.translate(x + w / 2, y + h / 2);
        ctx.rotate(rot || 0);
        ctx.fillStyle = c;
        ctx.fillRect(-w / 2, -h / 2, w, h);
        ctx.restore();
        break;
      }
      case "sprite": {
        const [, tex, x, y, w, h, uv, tint, rot] = op;
        const t = textures.get(tex);
        if (!t) break;
        const [u0, v0, du, dv] = uv || [0, 0, 1, 1];
        // A negative extent mirrors: draw from the far edge with a flipped scale.
        const sx = Math.min(u0, u0 + du) * t.w, sy = Math.min(v0, v0 + dv) * t.h;
        const sw = Math.abs(du) * t.w, sh = Math.abs(dv) * t.h;
        ctx.save();
        ctx.translate(x + w / 2, y + h / 2);
        ctx.rotate(rot || 0);
        ctx.scale(du < 0 ? -1 : 1, dv < 0 ? -1 : 1);
        ctx.globalAlpha = tint && tint.length === 4 ? tint[3] / 255 : 1;
        ctx.imageSmoothingEnabled = false;
        ctx.drawImage(t.bitmap, sx, sy, sw, sh, -w / 2, -h / 2, w, h);
        ctx.restore();
        break;
      }
      case "emitter":
        drawParticles(op[1]);
        break;
      default:
        break; // sound, music (heard in `hearFrame`), the effects (`paintFrame`), cursor-zone, cells, cells-rgb and anything newer
    }
  }
}

function roundRect(x, y, w, h, r, fill) {
  ctx.fillStyle = fill;
  if (r > 0 && ctx.roundRect) {
    ctx.beginPath();
    ctx.roundRect(x, y, w, h, r);
    ctx.fill();
  } else {
    ctx.fillRect(x, y, w, h);
  }
}

// The grid origin: like the runtime's `grid_origin`, the vertical remainder is centred.
// A frame's effects are passes around its ops: the scene (before the top-level
// `[:overlay]`), the particles of emitters it no longer carries, the light map, the
// overlay, then bloom and the CRT — the runtime's order.
function paintFrame(frame) {
  lastFrame = frame;
  stepParticles(frame);
  const fx = frameEffects(frame);
  const split = fx.overlayAt < 0 ? frame.length : fx.overlayAt;
  const rows = Math.floor(canvas.height / cell.h);
  const oy = Math.floor((canvas.height - rows * cell.h) / 2);
  ctx.fillStyle = BACKGROUND;
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  paintOps(frame.slice(0, split), 0, oy, cell.w, cell.h, 0);
  const carried = new Set();
  walkOps(frame, (op) => { if (op[0] === "emitter") carried.add(op[1]); });
  for (const id of emitters.keys()) if (!carried.has(id)) drawParticles(id);
  if (fx.ambient) applyLighting(fx);
  paintOps(frame.slice(split), 0, oy, cell.w, cell.h, 0);
  if (fx.bloom) applyBloom(fx.bloom);
  if (fx.crt) applyCrt(fx.crt);
  scheduleParticles();
}

// --- sizing --------------------------------------------------------------------------------------

function resize() {
  canvas.width = Math.max(1, Math.round(canvas.clientWidth * devicePixelRatio));
  canvas.height = Math.max(1, Math.round(canvas.clientHeight * devicePixelRatio));
  measureCell();
  paintFrame(lastFrame);
  sendResize();
}

function sendResize() {
  if (pixelInput) send(`[:resize ${canvas.width} ${canvas.height}]`);
  else send(`[:resize ${Math.floor(canvas.width / cell.w)} ${Math.floor(canvas.height / cell.h)}]`);
}

// --- input, as Brood forms ----------------------------------------------------------------------

function send(form) {
  if (tabId != null) post(form);
}

// A Brood string literal for a one-character key.
function lit(s) {
  return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"';
}

const NAMED = {
  ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right",
  Enter: "enter", Escape: "escape", Backspace: "backspace", Tab: "tab", Delete: "delete",
  Home: "home", End: "end", PageUp: "page-up", PageDown: "page-down", " ": null,
};

// The key a keyboard event names, in the runtime's vocabulary: a 1-char string for a
// printable key, a keyword for a named key or a chord — or null for a bare modifier.
function keyForm(e) {
  const mods = (e.ctrlKey ? "ctrl-" : "") + (e.altKey ? "alt-" : "");
  if (e.key in NAMED && NAMED[e.key] !== null) {
    const shift = e.shiftKey && e.key !== "Tab" ? "shift-" : "";
    return ":" + mods + shift + NAMED[e.key];
  }
  if (e.key === " ") return mods ? ":" + mods + "space" : lit(" ");
  if (e.key.length === 1) {
    if (mods) return ":" + mods + e.key.toLowerCase();
    return lit(e.key);
  }
  return null;
}

// Mouse positions in the unit the app asked for: pixels, or the cell under the pointer.
function mousePos(e) {
  const r = canvas.getBoundingClientRect();
  const px = Math.round((e.clientX - r.left) * devicePixelRatio);
  const py = Math.round((e.clientY - r.top) * devicePixelRatio);
  if (pixelInput) return [px, py];
  return [Math.floor(px / cell.w), Math.floor(py / cell.h)];
}

function mods(e) {
  const m = [];
  if (e.ctrlKey) m.push(":ctrl");
  if (e.altKey) m.push(":alt");
  if (e.shiftKey) m.push(":shift");
  return "[" + m.join(" ") + "]";
}

const BUTTONS = [":left", ":middle", ":right"];
let held = null;

canvas.addEventListener("keydown", (e) => {
  const k = keyForm(e);
  if (k == null) return;
  e.preventDefault();
  send(k);
});
canvas.addEventListener("keyup", (e) => {
  const k = keyForm(e);
  if (k == null) return;
  send(`[:key-up ${k}]`);
});
canvas.addEventListener("mousedown", (e) => {
  canvas.focus();
  const [x, y] = mousePos(e);
  held = BUTTONS[e.button] || ":left";
  send(`[:mouse :press ${held} ${y} ${x} ${mods(e)} ${e.detail || 1}]`);
});
canvas.addEventListener("mouseup", (e) => {
  const [x, y] = mousePos(e);
  const b = BUTTONS[e.button] || ":left";
  held = null;
  send(`[:mouse :release ${b} ${y} ${x} ${mods(e)}]`);
});
let lastMove = null;
canvas.addEventListener("mousemove", (e) => {
  const [x, y] = mousePos(e);
  const key = x + "," + y;
  if (key === lastMove) return; // once per pixel (or cell) crossed, as the window does
  lastMove = key;
  if (held) send(`[:mouse :drag ${held} ${y} ${x} ${mods(e)}]`);
  else send(`[:mouse :move nil ${y} ${x} ${mods(e)}]`);
});
canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  const [x, y] = mousePos(e);
  const dy = -e.deltaY / 100;
  send(`[:mouse ${dy > 0 ? ":scroll-up" : ":scroll-down"} nil ${y} ${x} ${mods(e)} ${dy.toFixed(3)}]`);
}, { passive: false });
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
window.addEventListener("resize", resize);

// --- the event stream ----------------------------------------------------------------------------

function decodeBase64(s) {
  const bin = atob(s);
  const out = new Uint8ClampedArray(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// The display's stream, by event name: the handlers below register here, and the transport
// at the end of the file feeds them — SSE for a served game, the page's own runtime for a
// game running in it.
const on = {};
const events = { addEventListener: (name, handler) => { on[name] = handler; } };
events.addEventListener("hello", (e) => {
  tabId = JSON.parse(e.data).id;
  status.textContent = "";
  resize();
  canvas.focus();
});
events.addEventListener("window", (e) => {
  const o = JSON.parse(e.data);
  if (o.input === "pixels") pixelInput = true;
  else if (o.input === "cells") pixelInput = false;
  if (o.title) document.title = o.title;
  sendResize();
});
events.addEventListener("texture", async (e) => {
  const t = JSON.parse(e.data);
  const data = new ImageData(decodeBase64(t.rgba), t.w, t.h);
  const bitmap = await createImageBitmap(data);
  textures.set(t.tex, { bitmap, w: t.w, h: t.h });
  paintFrame(lastFrame);
});
events.addEventListener("texture-free", (e) => {
  textures.delete(JSON.parse(e.data).tex);
});
events.addEventListener("frame", (e) => {
  const frame = JSON.parse(e.data);
  hearFrame(frame);
  paintFrame(frame);
});
events.addEventListener("error", (e) => {
  status.textContent = e.data ? JSON.parse(e.data).error : "disconnected";
});

measureCell();
resize();

// --- sound ---------------------------------------------------------------------------------------
// Browsers only start audio after a user gesture, so the context is made on the first
// key or click; a sound op before that is dropped, not queued.

function ensureAudio() {
  if (!audio && (window.AudioContext || window.webkitAudioContext)) {
    audio = new (window.AudioContext || window.webkitAudioContext)();
  }
  if (audio && audio.state === "suspended") audio.resume();
  // music asked for before the gesture starts now
  hearMusic(lastFrame);
}

// A frame's sounds play once, when it arrives — not when a resize or a late texture
// repaints it — and its music is reconciled, as the runtime's window does.
function hearFrame(frame) {
  burstsArrived(frame);
  for (const op of frame) {
    if (Array.isArray(op) && op[0] === "sound") playSound(op[1], op[2]);
  }
  hearMusic(frame);
}

// Music is state: the first `[:music snd vol]` op of the frame on screen keeps that sound
// looping; the same handle again only moves the volume, another switches tracks, none
// fades it out. The fade matches the runtime's 50 ms.
const FADE_SECONDS = 0.05;
let music = null; // { snd, source, gain }

function hearMusic(frame) {
  const op = frame.find((o) => Array.isArray(o) && o[0] === "music" && sounds.has(o[1]));
  const vol = op && op[2] > 0 ? Math.min(1, op[2]) : 0.18;
  if (music && op && music.snd === op[1]) {
    music.gain.gain.setTargetAtTime(vol, audio.currentTime, FADE_SECONDS / 3);
    return;
  }
  if (music) {
    music.gain.gain.setTargetAtTime(0, audio.currentTime, FADE_SECONDS / 3);
    music.source.stop(audio.currentTime + FADE_SECONDS * 2);
    music = null;
  }
  if (!op || !audio || audio.state !== "running") return;
  const source = audio.createBufferSource();
  source.buffer = sounds.get(op[1]);
  source.loop = true;
  const gain = audio.createGain();
  gain.gain.value = 0;
  gain.gain.setTargetAtTime(vol, audio.currentTime, FADE_SECONDS / 3);
  source.connect(gain).connect(audio.destination);
  source.start();
  music = { snd: op[1], source, gain };
}
canvas.addEventListener("keydown", ensureAudio);
canvas.addEventListener("mousedown", ensureAudio);

function playSound(snd, vol) {
  const buffer = sounds.get(snd);
  if (!audio || !buffer) return;
  const source = audio.createBufferSource();
  source.buffer = buffer;
  const gain = audio.createGain();
  gain.gain.value = vol && vol > 0 ? Math.min(1, vol) : 0.18;
  source.connect(gain).connect(audio.destination);
  source.start();
}

// A sound is decoded off the main thread: the base64 by a `data:` fetch, the PCM by the
// browser's own decoder, as the WAV it is once given a header — a track of half a megabyte
// no longer stalls the page that is drawing frames. Until it is ready, a frame playing it
// plays nothing (and music starts on the next frame after).
const decoding = new Set(); // handles in flight, so a free that overtakes a decode wins

// The 44-byte RIFF/WAVE header for `length` bytes of PCM16 at `rate` Hz.
function wavHeader(length, rate, channels) {
  const header = new DataView(new ArrayBuffer(44));
  const text = (offset, s) => [...s].forEach((c, i) => header.setUint8(offset + i, c.charCodeAt(0)));
  text(0, "RIFF");
  header.setUint32(4, 36 + length, true);
  text(8, "WAVE");
  text(12, "fmt ");
  header.setUint32(16, 16, true);
  header.setUint16(20, 1, true);
  header.setUint16(22, channels, true);
  header.setUint32(24, rate, true);
  header.setUint32(28, rate * channels * 2, true);
  header.setUint16(32, channels * 2, true);
  header.setUint16(34, 16, true);
  text(36, "data");
  header.setUint32(40, length, true);
  return header.buffer;
}

events.addEventListener("sound", async (e) => {
  const s = JSON.parse(e.data);
  if (!(window.AudioContext || window.webkitAudioContext)) return;
  audio = audio || new (window.AudioContext || window.webkitAudioContext)();
  decoding.add(s.snd);
  try {
    const pcm = await (await fetch(`data:application/octet-stream;base64,${s.pcm}`)).arrayBuffer();
    if (pcm.byteLength < 2) return;
    const wav = new Uint8Array(44 + pcm.byteLength);
    wav.set(new Uint8Array(wavHeader(pcm.byteLength, s.rate, s.channels)), 0);
    wav.set(new Uint8Array(pcm), 44);
    const buffer = await audio.decodeAudioData(wav.buffer);
    if (decoding.has(s.snd)) sounds.set(s.snd, buffer);
  } catch (error) {
    console.warn(`sound ${s.snd}: ${error}`);
  } finally {
    decoding.delete(s.snd);
  }
});
events.addEventListener("sound-free", (e) => {
  const snd = JSON.parse(e.data).snd;
  decoding.delete(snd);
  sounds.delete(snd);
});

events.addEventListener("bye", () => {
  status.textContent = "the game has ended";
});

// --- the transport -------------------------------------------------------------------------------
// A served game streams its display over SSE and takes input by POST (`editor/web`). A game
// running IN this page (`tab.html`: brood's wasm build, the game's bundle) sets
// `window.BROOD_TAB` first and hands both over directly.

function dispatch(name, data) {
  const handler = on[name];
  if (handler) handler({ data });
}

let post;
if (window.BROOD_TAB) {
  post = (form) => window.BROOD_TAB.input(form);
  window.BROOD_TAB.connect(dispatch);
} else {
  const source = new EventSource("/events");
  for (const name of Object.keys(on)) source.addEventListener(name, (e) => dispatch(name, e.data));
  source.onerror = () => { status.textContent = "disconnected"; };
  // One POST at a time, each after the last has landed: two in flight at once can reach the
  // bridge in either order, and a resize in cells overtaking the resize in pixels left a game
  // believing its window was 88×28 pixels.
  let sending = Promise.resolve();
  post = (form) => {
    const id = tabId;
    sending = sending
      .then(() => fetch(`/input/${id}`, { method: "POST", body: form, keepalive: true }))
      .catch(() => {});
  };
}
