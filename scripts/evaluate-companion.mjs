import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const fixture=JSON.parse(await readFile(path.join(root,'tests/fixtures/companion-policy.json'),'utf8'));
const checks=[
  {name:'memory_provenance_and_session_recovery',command:process.execPath,args:['tests/memory-system.test.mjs']},
  {name:'reminder_receipts_interruption_and_runtime_recovery',command:'cargo',args:['test','--locked','--manifest-path','src-tauri/runtime-contracts/Cargo.toml','-j','1']},
  {name:'prompt_scenarios_and_prompt_invariants',command:process.execPath,args:['scripts/evaluate-prompts.mjs']},
];
const results=[];
for(const check of checks){
  const start=performance.now();
  const exitCode=await new Promise((resolve,reject)=>{
    const child=spawn(check.command,check.args,{cwd:root,stdio:'inherit'});
    child.once('error',reject);child.once('exit',code=>resolve(code??1));
  });
  results.push({name:check.name,passed:exitCode===0,test_duration_ms:Math.round(performance.now()-start)});
}
const promptScenarios=JSON.parse(await readFile(path.join(root,'tests/fixtures/prompt-scenarios.json'),'utf8')).scenarios;
const report={mode:'offline_contracts',model_calls:0,real_model_quality_measured:false,policy_cases:fixture.length,prompt_scenarios:promptScenarios.length,checks:results};
await mkdir(path.join(root,'tmp'),{recursive:true});
const output=path.join(root,'tmp/companion-eval-report.json');
await writeFile(output,JSON.stringify(report,null,2)+'\n');
console.log(`Companion evaluation report: ${output}`);
process.exitCode=results.every(result=>result.passed)?0:1;
