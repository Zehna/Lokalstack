/**
 * Pure domain-adapter tests: process grouping and identity presentation
 * helpers (Phase 10A, spec §27). No Tauri, no React.
 */
import { describe, expect, it } from 'vitest'

import { groupListenersByProcess } from '@/features/services/groupProcesses'
import {
  categoryBadgeClass,
  confidenceBadgeClass,
  confidenceLabel,
  confidenceTooltip,
  evidenceSummary,
} from '@/features/services/identity'
import { makeProcess, makeService, makeProject } from '@/test/fixtures'
import type { PortListener } from '@/types/domain'

function listener(port: number, pid: number, address = '127.0.0.1', ipVersion: 4 | 6 = 4): PortListener {
  return { protocol: 'tcp', ipVersion, localAddress: address, port, pid, state: 'LISTEN' }
}

describe('groupListenersByProcess', () => {
  it('collapses multiple listener rows of one PID into a single group', () => {
    const groups = groupListenersByProcess(
      [listener(3000, 42), listener(3001, 42), listener(3000, 42, '::1', 6)],
      new Map([[42, makeProcess({ pid: 42 })]]),
    )
    expect(groups).toHaveLength(1)
    expect(groups[0].ports).toEqual([3000, 3001])
    // localeCompare puts '::1' before '127.0.0.1' — documented adapter behavior.
    expect(groups[0].addresses).toEqual(['::1', '127.0.0.1'])
  })

  it('keeps different PIDs separate and sorts ports ascending', () => {
    const groups = groupListenersByProcess(
      [listener(5173, 7), listener(3000, 9)],
      new Map([
        [7, makeProcess({ pid: 7 })],
        [9, makeProcess({ pid: 9 })],
      ]),
    )
    expect(groups.map((g) => g.pid).sort((a, b) => a - b)).toEqual([7, 9])
  })

  it('falls back honestly when the snapshot lacks the process', () => {
    const groups = groupListenersByProcess([listener(3000, 4242)], new Map())
    expect(groups[0].displayName).toBe('Unavailable')
    expect(groups[0].accessible).toBe(false)
    expect(groups[0].process).toBeNull()
  })

  it('prefers the service identity over the process name for display', () => {
    const groups = groupListenersByProcess(
      [listener(3000, 42)],
      new Map([[42, makeProcess({ pid: 42, name: 'node.exe' })]]),
      new Map([[42, { pid: 42, ...makeService({ displayName: 'Vite' }) }]]),
    )
    expect(groups[0].displayName).toBe('Vite')
  })

  it('attaches the project identity when a link exists', () => {
    const project = makeProject()
    const groups = groupListenersByProcess(
      [listener(3000, 42)],
      new Map([[42, makeProcess({ pid: 42 })]]),
      new Map(),
      new Map([[42, project]]),
    )
    expect(groups[0].project?.name).toBe('historyai')
  })

  it('ranks identified services before unknown ones', () => {
    const groups = groupListenersByProcess(
      [listener(3000, 1), listener(5432, 2)],
      new Map([
        [1, makeProcess({ pid: 1 })],
        [2, makeProcess({ pid: 2 })],
      ]),
      new Map([
        [1, { pid: 1, ...makeService({ category: 'unknown', displayName: 'Unknown' }) }],
        [2, { pid: 2, ...makeService({ category: 'database', displayName: 'PostgreSQL' }) }],
      ]),
    )
    expect(groups[0].pid).toBe(2)
    expect(groups[1].pid).toBe(1)
  })
})

describe('identity presentation helpers', () => {
  it('confidence badge classes differ per level and low stays quiet', () => {
    expect(confidenceBadgeClass('exact')).toContain('emerald')
    expect(confidenceBadgeClass('high')).toContain('sky')
    expect(confidenceBadgeClass('medium')).toContain('amber')
    expect(confidenceBadgeClass('low')).toContain('slate')
  })

  it('confidence labels are capitalized', () => {
    expect(confidenceLabel('exact')).toBe('Exact')
    expect(confidenceLabel('low')).toBe('Low')
  })

  it('confidence tooltips explain the level honestly', () => {
    expect(confidenceTooltip('exact')).toContain('executable')
    expect(confidenceTooltip('low')).toContain('Weak')
  })

  it('category badge classes are distinct per category', () => {
    const classes = new Set(
      (['frontend', 'backend', 'database', 'ai', 'infrastructure', 'unknown'] as const).map(
        categoryBadgeClass,
      ),
    )
    expect(classes.size).toBe(6)
  })

  it('evidence summary joins source: value pairs', () => {
    expect(
      evidenceSummary(
        makeService({
          evidence: [
            { source: 'process_name', value: 'postgres.exe' },
            { source: 'command_line', value: 'vite' },
          ],
        }),
      ),
    ).toBe('process_name: postgres.exe · command_line: vite')
  })
})
