// 彩蛋用的彩色小猫：主窗口和全屏小猫窗口（catrain.html）共用
// 小猫本身是深灰色，滤镜调不出颜色：在画布上把灰色的身体换成别的颜色，粉色脸颊和绿色眼睛保留
const CAT_HUES = [null, 0, 25, 45, 90, 140, 175, 200, 225, 265, 300, 330]; // null = 原来的灰猫
async function makeCatColors() {
  const img = new Image();
  img.src = "assets/cat-awake.png";
  await img.decode();
  const w = img.naturalWidth, h = img.naturalHeight;
  const base = document.createElement("canvas");
  base.width = w; base.height = h;
  const bctx = base.getContext("2d");
  bctx.drawImage(img, 0, 0);
  const src = bctx.getImageData(0, 0, w, h);
  const out = [];
  for (const hue of CAT_HUES) {
    if (hue === null) { out.push(img.src); continue; }
    const d = new Uint8ClampedArray(src.data);
    for (let i = 0; i < d.length; i += 4) {
      if (d[i + 3] === 0) continue;
      const r = d[i], g = d[i + 1], b = d[i + 2];
      const max = Math.max(r, g, b), min = Math.min(r, g, b);
      if (max - min > 24) continue; // 有颜色的像素（眼睛、脸颊）不动
      // 灰度 → 同一色相的深浅：越暗的地方颜色越深，保留像素画的明暗层次
      const l = 0.32 + (max / 255) * 0.75;
      const [nr, ng, nb] = hslToRgb(hue / 360, 0.7, Math.min(l, 0.85));
      d[i] = nr; d[i + 1] = ng; d[i + 2] = nb;
    }
    const c = document.createElement("canvas");
    c.width = w; c.height = h;
    c.getContext("2d").putImageData(new ImageData(d, w, h), 0, 0);
    out.push(c.toDataURL());
  }
  return out;
}
function hslToRgb(h, s, l) {
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
  const f = (t) => {
    t = (t + 1) % 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  return [f(h + 1 / 3), f(h), f(h - 1 / 3)].map((v) => Math.round(v * 255));
}
