import { readFile, mkdtemp } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';

export async function loadProductionRequests(directory) {
  const requests = JSON.parse(await readFile(path.join(directory, 'requests.json'), 'utf8'));
  if (!Array.isArray(requests) || !requests.length || requests.some(entry =>
    entry.assembly !== 'production_prompt_building_and_generation'
    || !entry.scenario_id || !entry.character || !Array.isArray(entry.request?.messages)
    || !entry.request.messages.length)) throw new Error('Invalid production request export');
  return requests;
}

export async function exportProductionRequests(root) {
  const directory = await mkdtemp(path.join(tmpdir(), 'vivian-prompt-requests-'));
  const data = await mkdtemp(path.join(tmpdir(), 'vivian-prompt-fixture-data-'));
  const status = await new Promise((resolve, reject) => {
    const child = spawn('cargo', ['test', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--lib', '-j', '1',
      'pipeline::prompt_eval::export_companion_evaluation_requests', '--', '--exact', '--test-threads=1'], {
      cwd: root, stdio: 'inherit',
      env: { ...process.env, VIVIAN_TEST_DATA_DIR: data, VIVIAN_PROMPT_EVAL_EXPORT_DIR: directory },
    });
    child.once('error', reject); child.once('exit', code => resolve(code ?? 1));
  });
  if (status !== 0) throw new Error(`Production request export failed (${status}); no approximate prompt fallback`);
  return { directory, requests: await loadProductionRequests(directory) };
}
