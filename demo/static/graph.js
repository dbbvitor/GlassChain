// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
// Federation graph. Canvas2D is the baseline renderer and the fallback: it
// draws edges, nodes, labels and the moving transaction dots. A WebGPU layer
// (dots only, on a transparent overlay) is attempted **only** when the
// measured baseline frame time misses the 10 ms p99 budget at the current
// load, or when explicitly forced for verification with `?renderer=webgpu`
// (`?renderer=canvas` pins the baseline). Adapter failure or device loss
// falls back automatically. The decision is measurement-driven, never a
// framework preference.

const ROLE_FILL = {
  manufacturer: "#1fa38c",
  distributor: "#3a7ac8",
  logistics: "#de8a36",
  pharmacy: "#955bc7",
  regulator: "#b6ad31",
  certifier: "#6e767a",
};
const EVIL_FILL = "#c93c3c";
const KIND_FILL = {
  lot: "#e3e844",
  custody: "#5ed6b2",
  offer: "#7aa8ff",
  purchase: "#ff9e64",
  process: "#c9d34a",
  attestation: "#ebdb8a",
};
const CANVAS_BG = "#0e1116";
const EDGE = "rgba(140, 160, 175, 0.16)";
const EDGE_HIGHLIGHT = "#6e79e4";
const TEXT = "#dfe5ea";
const TEXT_DIM = "#93a0ab";

const COLUMN_X = { manufacturer: 0.12, distributor: 0.38, logistics: 0.64, pharmacy: 0.88 };
const FLOW_PERIOD_MS = 2600;
const FRAME_BUDGET_MS = 10;
const FRAME_WINDOW = 600;
const DECISION_SAMPLES = 300;
const GPU_MAX_DOTS = 2048;

const params = new URLSearchParams(window.location.search);
const rendererOverride = params.get("renderer"); // "canvas" | "webgpu" | null

let staticCanvas = null;
let staticCtx = null;
let overlayCanvas = null;
let tooltipEl = null;
let handlers = {};
let dpr = 1;
let viewWidth = 0;
let viewHeight = 0;

let orgs = [];
let nodes = [];
let edges = [];
let status = "idle";
let hover = null;
let dragOffsets = {};
let dragging = null;
let dragMoved = false;
let highlight = null;
let sceneSignature = "";

let running = false;
let visible = true;
let frame = 0;
let frameTimes = [];
let animClock = 0;
let font = "11px system-ui";
let reduceMotion = false;
let staticDirty = true;

// One dot per committed transaction, keyed by transaction id: a dot starts at
// its origin when the transaction is first seen and flows to its destination
// exactly once. The keyed timeline is what keeps the picture stable — array
// indexes shift as the backend window slides, which used to make dots teleport
// or restart mid-edge.
const dotTimeline = new Map();
const seenTx = new Set();
const DOT_LIFETIME_MS = FLOW_PERIOD_MS * 1.15;

// Renderer state: "canvas" is the baseline; "webgpu" is the dot layer.
let backend = "canvas";
let gpu = null;
let decided = rendererOverride === "canvas";

const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
reduceMotion = motionQuery.matches;
motionQuery.addEventListener("change", () => {
  reduceMotion = motionQuery.matches;
  syncLoop();
});

const WGSL = `
struct Uniforms { resolution: vec2f };
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VsOut {
  @builtin(position) position: vec4f,
  @location(0) local: vec2f,
  @location(1) tint: vec4f,
};

@vertex
fn vs(@builtin(vertex_index) vertex: u32,
      @location(0) center: vec2f,
      @location(1) radius: f32,
      @location(2) tint: vec4f) -> VsOut {
  var corners = array<vec2f, 6>(
    vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(-1.0, 1.0),
    vec2f(-1.0, 1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
  );
  let corner = corners[vertex];
  let pixel = center + corner * vec2f(radius);
  var out: VsOut;
  out.position = vec4f(
    pixel.x / uniforms.resolution.x * 2.0 - 1.0,
    1.0 - pixel.y / uniforms.resolution.y * 2.0,
    0.0,
    1.0,
  );
  out.local = corner;
  out.tint = tint;
  return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
  let d = length(in.local);
  if (d > 1.0) { discard; }
  let edge = 1.0 - smoothstep(0.72, 1.0, d);
  return vec4f(in.tint.rgb, in.tint.a * edge);
}
`;

// ── Scene ──────────────────────────────────────────────────────────────────

function spread(index, count) {
  return count <= 1 ? 0.5 : index / (count - 1);
}

function buildScene(list) {
  const columns = { manufacturer: [], distributor: [], logistics: [], pharmacy: [] };
  const bottom = [];
  const evil = [];
  for (const org of list) {
    if (org.evil) evil.push(org);
    else if (org.role in columns) columns[org.role].push(org);
    else bottom.push(org);
  }
  const placed = [];
  for (const [role, members] of Object.entries(columns)) {
    members.forEach((org, index) => {
      placed.push({ id: org.id, role: org.role, evil: false, trust: org.trust_score, records: org.records, x: COLUMN_X[role], y: 0.18 + 0.64 * spread(index, members.length) });
    });
  }
  bottom.forEach((org, index) => {
    placed.push({ id: org.id, role: org.role, evil: false, trust: org.trust_score, records: org.records, x: 0.16 + 0.68 * spread(index, bottom.length), y: 0.92 });
  });
  evil.forEach((org, index) => {
    placed.push({ id: org.id, role: org.role, evil: true, trust: org.trust_score, records: org.records, x: 0.16 + 0.68 * spread(index, evil.length), y: 0.08 });
  });
  nodes = placed;
}

function nodePosition(node) {
  const offset = dragOffsets[node.id];
  return offset ? { x: offset.x, y: offset.y } : { x: node.x, y: node.y };
}

function nodeRadius() {
  return nodes.length > 22 ? 9 : nodes.length > 14 ? 11 : 13;
}

/** Adopt the backend's recent-transaction window into the keyed timeline. */
function syncTimeline(list, clock) {
  for (const tx of list) {
    if (seenTx.has(tx.id)) {
      const entry = dotTimeline.get(tx.id);
      if (entry) entry.tx = tx;
      continue;
    }
    seenTx.add(tx.id);
    dotTimeline.set(tx.id, { tx, startedAt: clock });
  }
  if (seenTx.size > 600) {
    for (const id of [...seenTx].slice(0, seenTx.size - 400)) seenTx.delete(id);
  }
  for (const [id, entry] of dotTimeline) {
    if (clock - entry.startedAt >= DOT_LIFETIME_MS) dotTimeline.delete(id);
  }
  while (dotTimeline.size > 240) {
    dotTimeline.delete(dotTimeline.keys().next().value);
  }
}

function dotPositions(clock) {
  const byId = Object.fromEntries(nodes.map((node) => [node.id, nodePosition(node)]));
  const dots = [];
  for (const entry of dotTimeline.values()) {
    const { tx } = entry;
    const from = byId[tx.from];
    const to = byId[tx.to];
    if (!from || !to) continue;
    // A stopped run (or reduced motion) freezes the picture at a fixed
    // progress, so a re-draw never makes motionless transactions jump.
    const progress =
      running && !reduceMotion
        ? Math.min(1.15, Math.max(0, (clock - entry.startedAt) / FLOW_PERIOD_MS))
        : 0.5;
    const travel = Math.min(1, progress);
    const alpha = progress > 0.85 ? Math.max(0, 1 - (progress - 0.85) / 0.3) : 1;
    if (tx.from === tx.to) {
      dots.push({ x: from.x, y: from.y, tx, self: true, alpha });
      continue;
    }
    dots.push({
      x: from.x + (to.x - from.x) * travel,
      y: from.y + (to.y - from.y) * travel,
      tx,
      self: false,
      alpha,
    });
  }
  return dots;
}

function cssColor(rgb) {
  const match = /^#([0-9a-f]{6})$/i.exec(rgb);
  if (!match) return [1, 1, 1, 1];
  const value = Number.parseInt(match[1], 16);
  return [((value >> 16) & 255) / 255, ((value >> 8) & 255) / 255, (value & 255) / 255, 1];
}

function dotColor(kind) {
  return KIND_FILL[kind] || "#e3e844";
}

// ── Canvas2D baseline ──────────────────────────────────────────────────────

function drawStaticCanvas() {
  const ctx = staticCtx;
  const width = viewWidth;
  const height = viewHeight;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = CANVAS_BG;
  ctx.fillRect(0, 0, width, height);

  const byId = Object.fromEntries(nodes.map((node) => [node.id, nodePosition(node)]));
  for (const edge of edges) {
    const from = byId[edge.from];
    const to = byId[edge.to];
    if (!from || !to) continue;
    const onPath =
      highlight &&
      highlight.includes(edge.from) &&
      highlight.includes(edge.to) &&
      Math.abs(highlight.indexOf(edge.from) - highlight.indexOf(edge.to)) === 1;
    ctx.strokeStyle = onPath ? EDGE_HIGHLIGHT : EDGE;
    ctx.lineWidth = onPath ? 3 : 1 + Math.min(5, Math.sqrt(edge.count) / 3);
    ctx.beginPath();
    ctx.moveTo(from.x * width, from.y * height);
    ctx.lineTo(to.x * width, to.y * height);
    ctx.stroke();
  }

  const radius = nodeRadius();
  ctx.font = font;
  ctx.textAlign = "center";
  for (const node of nodes) {
    const position = nodePosition(node);
    const cx = position.x * width;
    const cy = position.y * height;
    ctx.fillStyle = node.evil ? EVIL_FILL : ROLE_FILL[node.role] || "#808080";
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();
    if (hover && hover.type === "node" && hover.id === node.id) {
      ctx.strokeStyle = "#ffffff";
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(cx, cy, radius + 4, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.fillStyle = node.evil ? "#f0b4b4" : TEXT;
    const labelY = position.y > 0.85 ? cy - radius - 8 : cy + radius + 14;
    ctx.fillText(node.id, cx, labelY);
  }
}

function drawDotsCanvas(when) {
  const ctx = staticCtx;
  const width = viewWidth;
  const height = viewHeight;
  const radius = nodeRadius();
  for (const dot of dotPositions(when)) {
    const color = dotColor(dot.tx.kind);
    const cx = dot.x * width;
    const cy = dot.y * height;
    if (dot.self) {
      const pulse = reduceMotion ? 0.85 : 0.55 + Math.sin(when / 220) * 0.3;
      ctx.strokeStyle = color;
      ctx.globalAlpha = pulse * dot.alpha;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.arc(cx, cy, radius + 3, 0, Math.PI * 2);
      ctx.stroke();
      ctx.globalAlpha = 1;
    } else {
      ctx.globalAlpha = dot.alpha;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(cx, cy, 3.5, 0, Math.PI * 2);
      ctx.fill();
      if (hover && hover.type === "tx" && hover.tx === dot.tx) {
        ctx.strokeStyle = "#ffffff";
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(cx, cy, 6.5, 0, Math.PI * 2);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
    }
  }
}

// ── WebGPU dot layer ───────────────────────────────────────────────────────

async function tryWebGPU() {
  if (!navigator.gpu || !overlayCanvas) return null;
  try {
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    device.lost.then(() => {
      gpu = null;
      backend = "canvas";
      staticDirty = true;
      setRendererNote("Canvas2D fallback — WebGPU device was lost");
    });
    const context = overlayCanvas.getContext("webgpu");
    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({ device, format, alphaMode: "premultiplied" });
    const module = device.createShaderModule({ code: WGSL });
    const pipeline = device.createRenderPipeline({
      layout: "auto",
      vertex: {
        module,
        entryPoint: "vs",
        buffers: [{
          arrayStride: 28,
          attributes: [
            { shaderLocation: 0, offset: 0, format: "float32x2" },
            { shaderLocation: 1, offset: 8, format: "float32" },
            { shaderLocation: 2, offset: 12, format: "float32x4" },
          ],
        }],
      },
      fragment: {
        module,
        entryPoint: "fs",
        targets: [{
          format,
          blend: {
            color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" },
            alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
          },
        }],
      },
      primitive: { topology: "triangle-list" },
    });
    const uniforms = device.createBuffer({
      size: 16,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });
    const instances = device.createBuffer({
      size: GPU_MAX_DOTS * 28,
      usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
    });
    const bindGroup = device.createBindGroup({
      layout: pipeline.getBindGroupLayout(0),
      entries: [{ binding: 0, resource: { buffer: uniforms } }],
    });
    gpu = { device, context, format, pipeline, uniforms, instances, bindGroup };
    return gpu;
  } catch (error) {
    console.warn("WebGPU unavailable:", error);
    return null;
  }
}

function drawDotsGpu(when) {
  const dots = dotPositions(when).filter((dot) => !dot.self).slice(0, GPU_MAX_DOTS);
  if (dots.length) {
    const packed = new Float32Array(dots.length * 7);
    dots.forEach((dot, index) => {
      const color = cssColor(dotColor(dot.tx.kind));
      color[3] = dot.alpha;
      packed.set(
        [dot.x * viewWidth, dot.y * viewHeight, hover && hover.tx === dot.tx ? 6 : 4, ...color],
        index * 7,
      );
    });
    gpu.device.queue.writeBuffer(gpu.instances, 0, packed);
  }
  gpu.device.queue.writeBuffer(
    gpu.uniforms,
    0,
    new Float32Array([viewWidth, viewHeight, 0, 0]),
  );
  const encoder = gpu.device.createCommandEncoder();
  const pass = encoder.beginRenderPass({
    colorAttachments: [{
      view: gpu.context.getCurrentTexture().createView(),
      clearValue: { r: 0, g: 0, b: 0, a: 0 },
      loadOp: "clear",
      storeOp: "store",
    }],
  });
  pass.setPipeline(gpu.pipeline);
  pass.setBindGroup(0, gpu.bindGroup);
  pass.setVertexBuffer(0, gpu.instances);
  pass.draw(6, dots.length);
  pass.end();
  gpu.device.queue.submit([encoder.finish()]);
}

// ── Frame loop and the measured gate ───────────────────────────────────────

let rendererNote = "";

function setRendererNote(text) {
  rendererNote = text;
}

function frameStep(when) {
  if (staticCanvas && viewWidth > 0) {
    if (shouldAnimate()) animClock = when;
    const started = performance.now();
    if (backend === "webgpu" && gpu) {
      if (staticDirty) {
        drawStaticCanvas();
        staticDirty = false;
      }
      drawDotsGpu(animClock);
    } else {
      drawStaticCanvas();
      drawDotsCanvas(animClock);
    }
    frameTimes.push(performance.now() - started);
    if (frameTimes.length > FRAME_WINDOW) frameTimes.shift();
    maybeDecideRenderer();
  }
  frame = requestAnimationFrame(frameStep);
}

function percentile(values, pct) {
  if (!values.length) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * (pct / 100)))];
}

async function maybeDecideRenderer() {
  if (decided || frameTimes.length < DECISION_SAMPLES) return;
  decided = true;
  const baseline = percentile(frameTimes, 99);
  const forced = rendererOverride === "webgpu";
  if (!forced && baseline <= FRAME_BUDGET_MS) {
    setRendererNote(`Canvas2D — draw p99 ${baseline.toFixed(1)} ms within the ${FRAME_BUDGET_MS} ms budget`);
    return;
  }
  const attempted = await tryWebGPU();
  if (attempted) {
    backend = "webgpu";
    staticDirty = true;
    setRendererNote(
      forced
        ? "WebGPU dots (forced for verification; Canvas2D static layer)"
        : `WebGPU dots activated (Canvas2D p99 ${baseline.toFixed(1)} ms missed the ${FRAME_BUDGET_MS} ms budget)`,
    );
  } else {
    setRendererNote(
      `Canvas2D fallback — budget missed at p99 ${baseline.toFixed(1)} ms, no useful adapter`,
    );
  }
}

function startLoop() {
  if (frame) return;
  frame = requestAnimationFrame(frameStep);
}

function stopLoop() {
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
}

function shouldAnimate() {
  return running && visible && !reduceMotion && !document.hidden;
}

function renderOnce() {
  if (!visible || !staticCanvas || viewWidth <= 0) return;
  if (backend === "webgpu" && gpu) {
    drawStaticCanvas();
    drawDotsGpu(animClock);
  } else {
    drawStaticCanvas();
    drawDotsCanvas(animClock);
  }
  staticDirty = false;
}

function syncLoop() {
  if (shouldAnimate()) startLoop();
  else {
    stopLoop();
    renderOnce();
  }
}

// ── Sizing and interaction ─────────────────────────────────────────────────

function resize() {
  if (!staticCanvas) return;
  const rect = staticCanvas.getBoundingClientRect();
  dpr = Math.min(2, window.devicePixelRatio || 1);
  viewWidth = Math.max(1, Math.round(rect.width));
  viewHeight = Math.max(1, Math.round(rect.height));
  for (const canvas of [staticCanvas, overlayCanvas]) {
    if (!canvas) continue;
    canvas.width = Math.round(viewWidth * dpr);
    canvas.height = Math.round(viewHeight * dpr);
  }
  staticDirty = true;
}

function hitNode(point) {
  const radius = nodeRadius() + 8;
  let best = null;
  let bestDistance = Infinity;
  for (const node of nodes) {
    const position = nodePosition(node);
    const distance = Math.hypot((position.x - point.x) * viewWidth, (position.y - point.y) * viewHeight);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = node;
    }
  }
  return bestDistance <= radius ? best : null;
}

function hitTx(point) {
  let best = null;
  let bestDistance = Infinity;
  for (const dot of dotPositions(animClock)) {
    const distance = Math.hypot((dot.x - point.x) * viewWidth, (dot.y - point.y) * viewHeight);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = dot;
    }
  }
  return bestDistance <= 9 ? best : null;
}

function pointerPoint(event) {
  const rect = staticCanvas.getBoundingClientRect();
  return {
    x: (event.clientX - rect.left) / rect.width,
    y: (event.clientY - rect.top) / rect.height,
  };
}

function refreshHover(point) {
  const node = hitNode(point);
  if (node) {
    hover = { type: "node", id: node.id };
    const trust = node.records > 0 ? `trust ${node.trust}/100 (${node.records} records)` : "trust undefined (no records)";
    showTooltip(`${node.id} — ${node.role}${node.evil ? " (evil)" : ""} · ${trust} — click to inspect`, point);
    return;
  }
  const dot = hitTx(point);
  hover = dot && !dot.self ? { type: "tx", tx: dot.tx } : null;
  if (hover) {
    showTooltip(`${dot.tx.kind}: ${dot.tx.label} · h${dot.tx.height} — click for details`, point);
  } else {
    hideTooltip();
  }
}

function showTooltip(text, point) {
  if (!tooltipEl) return;
  tooltipEl.textContent = text.length > 96 ? `${text.slice(0, 95)}…` : text;
  tooltipEl.hidden = false;
  const wrapRect = staticCanvas.parentElement.getBoundingClientRect();
  const x = point.x * wrapRect.width;
  const y = point.y * wrapRect.height;
  tooltipEl.style.left = `${Math.min(wrapRect.width - 12, Math.max(12, x))}px`;
  tooltipEl.style.top = `${Math.max(10, y - 14)}px`;
  tooltipEl.style.transform = x > wrapRect.width / 2 ? "translate(-100%, -100%)" : "translate(8px, -100%)";
}

function hideTooltip() {
  if (tooltipEl) tooltipEl.hidden = true;
}

function attachInteractions() {
  staticCanvas.addEventListener("pointerdown", (event) => {
    const point = pointerPoint(event);
    const node = hitNode(point);
    if (!node) return;
    dragging = node.id;
    dragMoved = false;
    staticCanvas.setPointerCapture(event.pointerId);
    event.preventDefault();
  });

  staticCanvas.addEventListener("pointermove", (event) => {
    const point = pointerPoint(event);
    if (dragging) {
      dragMoved = true;
      dragOffsets[dragging] = {
        x: Math.min(1, Math.max(0, point.x)),
        y: Math.min(1, Math.max(0, point.y)),
      };
      staticDirty = true;
      if (!shouldAnimate()) renderOnce();
      return;
    }
    refreshHover(point);
    staticDirty = true;
    staticCanvas.style.cursor = hover ? "pointer" : "default";
    if (!shouldAnimate()) renderOnce();
  });

  const finish = (event) => {
    if (dragging) {
      const wasDragging = dragMoved;
      dragging = null;
      dragMoved = false;
      if (wasDragging) return;
    }
    refreshHover(pointerPoint(event));
    if (!hover) return;
    if (hover.type === "node") handlers.onNode?.(hover.id);
    else handlers.onTx?.(hover.tx);
  };
  staticCanvas.addEventListener("pointerup", finish);
  staticCanvas.addEventListener("pointerleave", () => {
    if (dragging) return;
    hover = null;
    hideTooltip();
    staticCanvas.style.cursor = "default";
    staticDirty = true;
    if (!shouldAnimate()) renderOnce();
  });
}

// ── Public API ─────────────────────────────────────────────────────────────

/** Wire the graph once; handlers receive node / transaction clicks. */
export function initGraph({ canvas, overlay, tooltip }, clickHandlers) {
  staticCanvas = canvas;
  overlayCanvas = overlay;
  tooltipEl = tooltip;
  handlers = clickHandlers;
  staticCtx = staticCanvas.getContext("2d");
  font = `11px ${getComputedStyle(document.body).fontFamily}`;
  animClock = performance.now();
  resize();
  attachInteractions();
  if (window.ResizeObserver) {
    new ResizeObserver(() => {
      resize();
      if (!shouldAnimate()) renderOnce();
    }).observe(staticCanvas);
  } else {
    window.addEventListener("resize", () => {
      resize();
      if (!shouldAnimate()) renderOnce();
    });
  }
  document.addEventListener("visibilitychange", syncLoop);
  if (rendererOverride === "webgpu") {
    decided = true;
    setRendererNote("WebGPU dots (forced for verification; Canvas2D static layer)");
    tryWebGPU().then((context) => {
      if (context) {
        backend = "webgpu";
        staticDirty = true;
      } else {
        setRendererNote("Canvas2D — forced WebGPU requested but no adapter was available");
      }
      syncLoop();
    });
  }
  syncLoop();
}

/** Feed the latest public snapshot into the scene. */
export function updateGraph(snapshot) {
  if (!staticCanvas) return;
  const nextOrgs = snapshot.orgs || [];
  const signature = JSON.stringify(nextOrgs.map((org) => [org.id, org.trust_score, org.records]));
  if (signature !== sceneSignature) {
    sceneSignature = signature;
    orgs = nextOrgs;
    buildScene(orgs);
    staticDirty = true;
  }
  edges = snapshot.edges || [];
  syncTimeline(snapshot.transactions || [], animClock);
  status = snapshot.status || "idle";
  running = status === "running" || status === "rebuilding";
  staticDirty = true;
  syncLoop();
}

/** Pause the loop while the graph is off-screen. */
export function setGraphVisible(isVisible) {
  visible = isVisible;
  syncLoop();
}

/** Accent a lot's custody path while its drawer is open. */
export function setHighlight(custodians) {
  highlight = custodians && custodians.length ? custodians : null;
  staticDirty = true;
  syncLoop();
}

/** Measured draw-time p99 and the active backend — the renderer-policy evidence. */
export function drawStats() {
  const stats = {
    p99: percentile(frameTimes, 99),
    samples: frameTimes.length,
    budget: FRAME_BUDGET_MS,
    backend: backend === "webgpu" ? "Canvas2D static + WebGPU dots" : "Canvas2D",
    note: rendererNote,
  };
  return stats;
}
