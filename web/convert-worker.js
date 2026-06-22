// Module worker: runs OBJ→OBJV conversion off the main thread so the UI stays
// responsive (animated progress) while a large file is parsed and compressed.
// `__BUILD__` is replaced with the short commit SHA at deploy time (cache-bust).
import init, { convert_obj } from './pkg/objv_viewer.js?v=__BUILD__';

const ready = init('./pkg/objv_viewer_bg.wasm?v=__BUILD__'); // this worker's own wasm

self.onmessage = async (e) => {
  const { buf, quantize } = e.data;
  try {
    await ready;
    self.postMessage({ type: 'stage', msg: 'converting' });
    const t0 = performance.now();
    const objv = convert_obj(new Uint8Array(buf), quantize);
    const ms = Math.round(performance.now() - t0);
    // Transfer the result back (zero-copy) rather than cloning it.
    self.postMessage({ type: 'done', objv, ms }, [objv.buffer]);
  } catch (err) {
    self.postMessage({ type: 'error', msg: String(err && err.message ? err.message : err) });
  }
};
