// Generates src-tauri/icons/icon.png — a 512x512 NovaProxy mark (dark rounded
// square, green disc, white play glyph) with no external deps. Run: node scripts/gen-icon.mjs
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";

const S = 512;
const buf = Buffer.alloc(S * S * 4);

const set = (x, y, r, g, b, a) => {
  const i = (y * S + x) * 4;
  // simple src-over onto existing pixel
  const da = buf[i + 3] / 255;
  const sa = a / 255;
  const oa = sa + da * (1 - sa);
  const blend = (sc, dc) => (oa === 0 ? 0 : Math.round((sc * sa + dc * (da * (1 - sa))) / oa));
  buf[i] = blend(r, buf[i]);
  buf[i + 1] = blend(g, buf[i + 1]);
  buf[i + 2] = blend(b, buf[i + 2]);
  buf[i + 3] = Math.round(oa * 255);
};

// rounded-square background
const radius = 112;
const inRounded = (x, y) => {
  const nx = Math.min(x, S - 1 - x);
  const ny = Math.min(y, S - 1 - y);
  if (nx >= radius || ny >= radius) return true;
  const dx = radius - nx;
  const dy = radius - ny;
  return dx * dx + dy * dy <= radius * radius;
};

const cx = S / 2;
const cy = S / 2;
const discR = 168;
// white play triangle (pointing right), roughly centered
const tri = (x, y) => {
  const px = x - (cx - 42);
  const py = y - cy;
  if (px < 0 || px > 150) return false;
  const half = 120 * (1 - px / 150);
  return py >= -half && py <= half;
};

for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    if (!inRounded(x, y)) continue;
    // base dark panel
    set(x, y, 27, 27, 29, 255);
    const d = Math.hypot(x - cx, y - cy);
    if (d <= discR) set(x, y, 40, 200, 64, 255); // #28c840 green disc
    if (tri(x, y)) set(x, y, 27, 27, 29, 255); // knock out play glyph in dark
  }
}

// encode PNG (truecolor + alpha, 8-bit)
const crcTable = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
const crc32 = (b) => {
  let c = 0xffffffff;
  for (let i = 0; i < b.length; i++) c = crcTable[(c ^ b[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
};

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
// raw scanlines with filter byte 0
const raw = Buffer.alloc(S * (S * 4 + 1));
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0;
  buf.copy(raw, y * (S * 4 + 1) + 1, y * S * 4, (y + 1) * S * 4);
}
const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync(new URL("../src-tauri/icons/", import.meta.url), { recursive: true });
writeFileSync(new URL("../src-tauri/icons/icon.png", import.meta.url), png);
console.log("wrote src-tauri/icons/icon.png", png.length, "bytes");
