// 生成 Tauri 图标源图（1024×1024 RGBA PNG），零第三方依赖。
// 视觉基调取自 UI.md：暖橙 #e8875a + 深底 #17181c，抽象终端提示符构图。
// 运行：node scripts/dev/make-icon.mjs
import { crc32, deflateSync } from 'node:zlib'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const W = 1024
const H = 1024
const px = Buffer.alloc(W * H * 4)

function set(x, y, [r, g, b, a]) {
  if (x < 0 || y < 0 || x >= W || y >= H) return
  const i = (y * W + x) * 4
  px[i] = r
  px[i + 1] = g
  px[i + 2] = b
  px[i + 3] = a
}

function fill(x0, y0, w, h, c) {
  for (let y = y0; y < y0 + h; y++) {
    for (let x = x0; x < x0 + w; x++) set(x, y, c)
  }
}

function roundedRect(x0, y0, w, h, r, c) {
  for (let y = y0; y < y0 + h; y++) {
    for (let x = x0; x < x0 + w; x++) {
      const dx = Math.max(x0 + r - x, 0, x - (x0 + w - 1 - r))
      const dy = Math.max(y0 + r - y, 0, y - (y0 + h - 1 - r))
      if (dx * dx + dy * dy <= r * r) set(x, y, c)
    }
  }
}

const ORANGE = [232, 135, 90, 255]
const ORANGE_HOV = [243, 155, 112, 255]
const DARK = [23, 24, 28, 255]

// 全幅暖橙底
fill(0, 0, W, H, ORANGE)
// 居中深色圆角方（终端面板隐喻）
roundedRect(96, 96, W - 192, H - 192, 150, DARK)
// 提示符：长条 + 短条 + 光标
roundedRect(236, 356, 320, 68, 30, ORANGE)
roundedRect(236, 500, 150, 68, 30, ORANGE)
roundedRect(640, 500, 128, 68, 30, ORANGE_HOV)

// ---- PNG 编码 ----
function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body) >>> 0)
  return Buffer.concat([len, body, crc])
}

const ihdr = Buffer.alloc(13)
ihdr.writeUInt32BE(W, 0)
ihdr.writeUInt32BE(H, 4)
ihdr[8] = 8 // bit depth
ihdr[9] = 6 // color type RGBA
// compression / filter / interlace = 0

// 每行前置 filter byte 0
const raw = Buffer.alloc(H * (1 + W * 4))
for (let y = 0; y < H; y++) {
  px.copy(raw, y * (1 + W * 4) + 1, y * W * 4, (y + 1) * W * 4)
}

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
])

const out = join(dirname(fileURLToPath(import.meta.url)), 'icon-source.png')
mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, png)
console.log(`written ${out} (${png.length} bytes)`)
