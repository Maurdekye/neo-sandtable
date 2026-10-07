import process from 'node:process'
import console from 'node:console'
import { spawnSync } from 'node:child_process'
const candidates = process.env.PYTHON
  ? [[process.env.PYTHON, []]]
  : process.platform === 'win32'
    ? [
        ['py', ['-3.12']],
        ['py', ['-3.14']],
        ['python', []],
      ]
    : [
        ['python3', []],
        ['python', []],
      ]
let runner
for (const [program, args] of candidates) {
  const probe = spawnSync(
    program,
    [...args, '-c', 'import sys,tomllib;sys.exit(sys.version_info<(3,11))'],
    { stdio: 'ignore' },
  )
  if (probe.status === 0) {
    runner = [program, args]
    break
  }
}
if (!runner) {
  console.error(
    'Viewer export requires Python 3.11 or later (set PYTHON to its executable).',
  )
  process.exit(1)
}
const result = spawnSync(runner[0], [...runner[1], 'tools/export_viewer.py'], {
  stdio: 'inherit',
})
process.exit(result.status ?? 1)
