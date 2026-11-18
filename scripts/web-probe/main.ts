import { CubismFramework, Option } from '@framework/live2dcubismframework';
import { CubismMatrix44 } from '@framework/math/cubismmatrix44';

import { WebModel } from './model';

const overlay = document.createElement('pre');
overlay.style.cssText =
  'position:fixed;left:0;top:0;margin:0;padding:8px;max-width:640px;white-space:pre-wrap;' +
  'font:12px/1.4 monospace;color:#ff9f9f;background:rgba(0,0,0,.72);pointer-events:none;display:none;z-index:9';
document.body.appendChild(overlay);

const report = (text: string): void => {
  overlay.style.display = 'block';
  overlay.textContent = text;
  document.title = `js error: ${text.split('\n')[0]}`;
};

window.addEventListener('error', event => report(String(event.message) + '\n' + String(event.error?.stack ?? '')));
window.addEventListener(
  'unhandledrejection',
  event => report(String((event as PromiseRejectionEvent).reason?.stack ?? (event as PromiseRejectionEvent).reason))
);

const canvas = document.getElementById('view') as HTMLCanvasElement;
const pixelRatio = Math.max(1, window.devicePixelRatio || 1);
canvas.width = Math.round(640 * pixelRatio);
canvas.height = Math.round(640 * pixelRatio);

const gl = canvas.getContext('webgl2', {
  alpha: true,
  antialias: true,
  premultipliedAlpha: true,
  preserveDrawingBuffer: false
});

if (!gl) {
  throw new Error('webgl2 unavailable');
}

CubismFramework.startUp(new Option());
CubismFramework.initialize();

const probe = {
  frames: 0,
  ready: false,
  lastDeltaMs: 0,
  averageDeltaMs: 0
};

(window as unknown as { probe: typeof probe }).probe = probe;

const model = new WebModel(canvas, gl);
const params = new URLSearchParams(location.search);
const home = params.get('dir') ?? '/res/Hiyori/';
const json = params.get('json') ?? 'Hiyori.model3.json';
const fill = Number(params.get('fill') ?? '0.925');
const culling = params.get('culling') !== '0';
const maskSize = Number(params.get('mask') ?? '1024');
const maskBuffers = Number(params.get('maskrt') ?? '1');

await model.load(home, json, maskSize, maskBuffers);

if (!culling) {
  (model.getModel() as unknown as { getDrawableCulling: () => boolean }).getDrawableCulling = () => false;
}

probe.ready = true;
document.documentElement.dataset.ready = '1';
console.log(`loaded ${home}${json} in ${performance.now().toFixed(0)} ms`);

if (params.get('debug') === '1') {
  const debugModel = model.getModel();
  overlay.style.display = 'block';
  overlay.textContent =
    `url ${location.search}\n` +
    `canvas ${canvas.width}x${canvas.height} px, model canvas ` +
    `${debugModel.getCanvasWidth().toFixed(1)}x${debugModel.getCanvasHeight().toFixed(1)} units, ` +
    `drawables ${debugModel.getDrawableCount()}, masking ${debugModel.isUsingMasking()}, ` +
    `mask rt ${model.getRenderer().getClippingMaskBufferSize()} x${model.getRenderer().getRenderTextureCount()}, ` +
    `culling ${culling}, fill ${fill}`;
}

canvas.addEventListener('pointermove', event => {
  const bounds = canvas.getBoundingClientRect();
  const x = ((event.clientX - bounds.left) / bounds.width) * 2 - 1;
  const y = -(((event.clientY - bounds.top) / bounds.height) * 2 - 1);

  model.setDragging(Math.max(-1, Math.min(1, x)), Math.max(-1, Math.min(1, y)));
});

let previous = performance.now();

const frame = (now: number): void => {
  const delta = Math.min((now - previous) / 1000, 0.1);
  previous = now;

  probe.lastDeltaMs = delta * 1000;
  probe.averageDeltaMs = probe.frames === 0 ? probe.lastDeltaMs : probe.averageDeltaMs * 0.95 + probe.lastDeltaMs * 0.05;

  const { width, height } = canvas;
  const aspect = width / height;
  const modelCanvas = model.getModel();
  const scale = (2 * fill) / Math.max(modelCanvas.getCanvasWidth() / aspect, modelCanvas.getCanvasHeight());

  const projection = new CubismMatrix44();
  projection.scale(scale / aspect, scale);

  gl.viewport(0, 0, width, height);
  gl.clearColor(0, 0, 0, 0);
  gl.clear(gl.COLOR_BUFFER_BIT);

  try {
    model.update(delta);
    model.draw(projection);
  } catch (error) {
    if (!document.title.startsWith('js error')) {
      report(String((error as Error).stack ?? error));
    }
  }

  probe.frames++;
  requestAnimationFrame(frame);
};

requestAnimationFrame(frame);
