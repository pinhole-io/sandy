import assert from 'node:assert/strict'
import { backendInfo, runShell } from '../index.js'

assert.ok(backendInfo().some((backend) => backend.id === 'auto'))

await assert.rejects(
  runShell({ command: '   ', workspace: process.cwd() }),
  /command must not be empty/,
)
await assert.rejects(
  runShell({ command: 'printf test', workspace: `${process.cwd()}/missing-workspace` }),
  /workspace must be an existing directory/,
)
await assert.rejects(
  runShell({ command: 'printf test', workspace: process.cwd(), maxOutputBytes: 0 }),
  /max_output_bytes must be positive/,
)

if (backendInfo().find((backend) => backend.id === 'auto')?.available) {
  const result = await runShell({
    command: 'printf sandy-js',
    workspace: process.cwd(),
    backend: 'auto',
    timeoutSecs: 10,
    maxOutputBytes: 64,
  })

  assert.equal(result.stdout, 'sandy-js')
  assert.equal(result.stderr, '')
  assert.equal(result.exitCode, 0)
  assert.equal(result.stdoutTruncated, false)
  assert.equal(result.stderrTruncated, false)
} else {
  await assert.rejects(
    runShell({ command: 'printf sandy-js', workspace: process.cwd(), backend: 'auto' }),
    /sandbox backend `auto` is unavailable/,
  )
}
