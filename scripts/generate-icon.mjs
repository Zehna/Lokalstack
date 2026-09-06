/**
 * Generates `src-tauri/app-icon.png` (1024x1024) — the source image that
 * `tauri icon` turns into all platform icons (`npm run icon`).
 *
 * Uses only Node built-ins: renders a rounded dark square with three
 * stacked "layers" bars (the LocalStack motif) via simple signed-distance
 * shapes with soft edges, then encodes a minimal RGBA PNG.
 */
import { deflateSync } from 'node:zlib'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const SIZE = 1024
const outPath = resolve(dirname(fileURLToPath(import.meta.url)), '../src-tauri/app-icon.png')

// ---------------------------------------------------------------------------
// Minimal PNG encoder (RGBA 8-bit, filter 0, no interlace)
// ---------------------------------------------------------------------------
const crcTable = (() => {
  const table = new Uint32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    }
    table[n] = c >>> 0
  }
  return table
})()

function crc32(bytes) {
  let c = 0xffffffff
  for (const b of bytes) {
    c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8)
  }
  return (c ^ 0xffffffff) >>> 0
}

function chunk(type, data) {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length, 0)
  const typeBuf = Buffer.from(type, 'ascii')
  const crcBuf = Buffer.alloc(4)
  crcBuf.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0)
  return Buffer.concat([length, typeBuf, data, crcBuf])
}

function encodePng(width, height, rgba) {
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // color type: RGBA
  const stride = width * 4
  const raw = Buffer.alloc((stride + 1) * height)
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0 // filter: none
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride)
  }
  const idat = deflateSync(raw, { level: 9 })
  return Buffer.concat([
    signature,
    chunk('IHDR', ihdr),
    chunk('IDAT', idat),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

// ---------------------------------------------------------------------------
// Icon rendering
// ---------------------------------------------------------------------------
const clamp01 = (v) => Math.max(0, Math.min(1, v))
const mix = (a, b, t) => a + (b - a) * t
const lerpColor = (c1, c2, t) => [mix(c1[0], c2[0], t), mix(c1[1], c2[1], t), mix(c1[2], c2[2], t)]

/** Signed distance to a rounded rectangle centered at (cx, cy). */
function sdRoundedRect(px, py, cx, cy, halfW, halfH, radius) {
  const qx = Math.abs(px - cx) - (halfW - radius)
  const qy = Math.abs(py - cy) - (halfH - radius)
  const outsideX = Math.max(qx, 0)
  const outsideY = Math.max(qy, 0)
  return Math.min(Math.max(qx, qy), 0) + Math.hypot(outsideX, outsideY) - radius
}

/** Anti-aliased coverage from a signed distance (positive = outside). */
function coverage(distance, aa) {
  return clamp01(0.5 - distance / aa)
}

const bgTop = [11, 18, 32] // dark navy
const bgBottom = [16, 32, 58]
const barColors = [
  [56, 189, 248], // sky-400
  [45, 212, 191], // teal-400
  [52, 211, 153], // emerald-400
]
const barCentersY = [0.5 - 0.13, 0.5, 0.5 + 0.13]
const AA = 3 / SIZE // anti-aliasing in normalized units

const rgba = Buffer.alloc(SIZE * SIZE * 4)

for (let y = 0; y < SIZE; y++) {
  const ny = y / (SIZE - 1)
  for (let x = 0; x < SIZE; x++) {
    const nx = x / (SIZE - 1)

    const dBg = sdRoundedRect(nx, ny, 0.5, 0.5, 0.5, 0.5, 0.18)
    const aBg = coverage(dBg, AA)
    let r = 0
    let g = 0
    let b = 0
    let a = aBg

    if (aBg > 0) {
      ;[r, g, b] = lerpColor(bgTop, bgBottom, ny)
      for (let i = 0; i < barCentersY.length; i++) {
        const dBar = sdRoundedRect(nx, ny, 0.5, barCentersY[i], 0.27, 0.045, 0.045)
        const aBar = coverage(dBar, AA)
        if (aBar > 0) {
          const [br, bg2, bb] = barColors[i]
          r = mix(r, br, aBar)
          g = mix(g, bg2, aBar)
          b = mix(b, bb, aBar)
        }
      }
    }

    const index = (y * SIZE + x) * 4
    rgba[index] = Math.round(r)
    rgba[index + 1] = Math.round(g)
    rgba[index + 2] = Math.round(b)
    rgba[index + 3] = Math.round(a * 255)
  }
}

mkdirSync(dirname(outPath), { recursive: true })
writeFileSync(outPath, encodePng(SIZE, SIZE, rgba))
console.log(`Wrote ${outPath}`)
