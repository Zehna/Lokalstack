/**
 * Phase 11C Task 19 — GitHub Safe Share issue workflow (spec §24).
 * The backend owns the fixed issue URL and the Safe-Share-transformed
 * title/body; the frontend only displays them and opens the backend-provided
 * URL after explicit user confirmation. No GitHub API, no PAT, no upload.
 */
// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

const { prepareMock, exportMock, revealMock } = vi.hoisted(() => ({
  prepareMock: vi.fn(),
  exportMock: vi.fn(),
  revealMock: vi.fn(),
}))

vi.mock('@/services/native/diagnostics', () => ({
  prepareLocalstackGithubIssue: prepareMock,
  exportSupportBundle: exportMock,
  revealExportResult: revealMock,
}))

import { GitHubIssueDialog } from './GitHubIssueDialog'

const draft = {
  title: '[Safe Share] severe incident docker:DOCKER_PIPE',
  body: 'LocalStack Control Center 1.0.1\nsubsystem: docker\nFingerprint: fp-123',
  issueUrl: 'https://github.com/Zehna/Lokalstack/issues/new?title=%5BSafe%20Share%5D&body=x',
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.unstubAllGlobals()
})

describe('GitHubIssueDialog', () => {
  it('opens only the backend-provided fixed GitHub URL', async () => {
    const user = userEvent.setup()
    const openSpy = vi.fn()
    vi.stubGlobal('open', openSpy)
    prepareMock.mockResolvedValue(draft)
    exportMock.mockResolvedValue({ exportId: 'exp-1', fileName: 'bundle.zip', profile: 'safe-share' })

    render(<GitHubIssueDialog bundleId="b-1" onClose={() => {}} />)
    await waitFor(() => expect(prepareMock).toHaveBeenCalledWith('b-1'))

    // Confirm opens the backend-provided URL and runs the Safe Share export.
    await user.click(screen.getByRole('button', { name: /continue/i }))
    await waitFor(() => expect(openSpy).toHaveBeenCalledWith(draft.issueUrl, '_blank', 'noopener,noreferrer'))
    expect(openSpy).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(exportMock).toHaveBeenCalledWith('b-1', 'safe-share', false))
    await waitFor(() => expect(revealMock).toHaveBeenCalledWith('exp-1'))
  })

  it('failure to prepare shows typed error and never navigates', async () => {
    const openSpy = vi.fn()
    vi.stubGlobal('open', openSpy)
    prepareMock.mockRejectedValue(new Error('PREPARE_FAILED: storage unavailable'))

    render(<GitHubIssueDialog bundleId="b-2" onClose={() => {}} />)
    const err = await screen.findByText(/storage unavailable/i)
    expect(err).toBeTruthy()
    expect(openSpy).not.toHaveBeenCalled()
    expect(exportMock).not.toHaveBeenCalled()
  })

  it('cancel leaves no partial export', async () => {
    const user = userEvent.setup()
    const openSpy = vi.fn()
    vi.stubGlobal('open', openSpy)
    prepareMock.mockResolvedValue(draft)

    render(<GitHubIssueDialog bundleId="b-3" onClose={() => {}} />)
    await screen.findByRole('dialog', { name: /create github issue/i })
    await user.click(screen.getByRole('button', { name: /cancel/i }))

    expect(openSpy).not.toHaveBeenCalled()
    expect(exportMock).not.toHaveBeenCalled()
    expect(revealMock).not.toHaveBeenCalled()
  })

  it('export failure is shown and stops the flow before reveal', async () => {
    const user = userEvent.setup()
    const openSpy = vi.fn()
    vi.stubGlobal('open', openSpy)
    prepareMock.mockResolvedValue(draft)
    exportMock.mockRejectedValue(new Error('EXPORT_CANCELLED: no destination chosen'))

    render(<GitHubIssueDialog bundleId="b-4" onClose={() => {}} />)
    await user.click(screen.getByRole('button', { name: /continue/i }))
    const err = await screen.findByText(/no destination chosen/i)
    expect(err).toBeTruthy()
    expect(revealMock).not.toHaveBeenCalled()
  })
})
