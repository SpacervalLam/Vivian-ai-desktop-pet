// Application-owned process. The worker is a cancellation boundary, not a security sandbox.
import { Worker } from 'node:worker_threads';
import { createInterface } from 'node:readline';

const write = value => process.stdout.write(JSON.stringify(value) + '\n');
const worker = new Worker(new URL('./task.mjs', import.meta.url), {
  workerData: { artifacts: process.argv[2] }, stdout: true, stderr: true,
  resourceLimits:{maxOldGenerationSizeMb:256,maxYoungGenerationSizeMb:32,stackSizeMb:8},
});
// User program output must never become transport messages.
worker.stdout.on('data', () => {});
worker.stderr.on('data', () => {});
worker.on('message', write);
worker.on('error', error => { write({kind:'fatal', error:error.message}); process.exitCode=1; });
worker.on('exit', () => process.exit());
createInterface({input:process.stdin, crlfDelay:Infinity}).on('line', line => {
  try { worker.postMessage(JSON.parse(line)); }
  catch { write({kind:'fatal',error:'Invalid browser runtime message'}); }
}).on('close', () => worker.terminate());
