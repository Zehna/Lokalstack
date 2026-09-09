import { describe, expect, it } from 'vitest'

import {
  clampPercent,
  formatBytes,
  formatCpuPercent,
  formatPort,
  formatTime,
} from '@/utils/format'

describe('formatBytes', () => {
  it('renders null and non-finite as an em dash', () => {
    expect(formatBytes(null)).toBe('—')
    expect(formatBytes(Number.NaN)).toBe('—')
    expect(formatBytes(-1)).toBe('—')
  })

  it('stays in bytes below 1024', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1023 B')
  })

  it('scales into larger units with one decimal', () => {
    expect(formatBytes(1024)).toBe('1 KB')
    expect(formatBytes(120_000_000)).toBe('114.4 MB')
    expect(formatBytes(6_100_000_000)).toBe('5.7 GB')
  })

  it('caps the unit at TB and keeps counting within it', () => {
    // 5 PiB renders as 5120 TB — the unit stops scaling, the number does not lie.
    expect(formatBytes(5 * 1024 ** 5)).toBe('5120 TB')
  })
})

describe('clampPercent', () => {
  it('clamps into 0–100 and sanitizes NaN', () => {
    expect(clampPercent(-5)).toBe(0)
    expect(clampPercent(50)).toBe(50)
    expect(clampPercent(150)).toBe(100)
    expect(clampPercent(Number.NaN)).toBe(0)
  })
})

describe('formatPort', () => {
  it('renders :port or an em dash', () => {
    expect(formatPort(3000)).toBe(':3000')
    expect(formatPort(null)).toBe('—')
  })
})

describe('formatCpuPercent', () => {
  it('never fabricates 0% on a missing sample', () => {
    expect(formatCpuPercent(null)).toBe('—')
    expect(formatCpuPercent(Number.NaN)).toBe('—')
    expect(formatCpuPercent(12.34)).toBe('12.3%')
  })
})

describe('formatTime', () => {
  it('renders an epoch timestamp as a locale time', () => {
    expect(formatTime(new Date('2024-01-01T10:20:30').getTime())).toBe(
      new Date('2024-01-01T10:20:30').toLocaleTimeString(),
    )
    expect(formatTime(null)).toBe('—')
  })
})
