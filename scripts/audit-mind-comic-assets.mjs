import {readFileSync,readdirSync,statSync,writeFileSync} from 'node:fs';
import {join,relative} from 'node:path';
import {comicMarkup} from '../src/components/mind-inspector/pages/comicMarkup.js';
import {bookSpreads} from '../src/components/mind-inspector/pages/comicBookStructure.js';
const walk=dir=>readdirSync(dir,{withFileTypes:true}).flatMap(e=>e.isDirectory()?walk(join(dir,e.name)):[join(dir,e.name)]);
const base='public/mind-comic';
const character={name:'Character',thought:'Thought',goal:'Goal',presence:'Online',mood:'Calm',emotions:[],needs:[],relations:[]};
const markup=bookSpreads.map(({mind})=>comicMarkup({vivian:character,nana:character},mind,'/mind-comic/')).join('\n');
const required=new Set([...markup.matchAll(/\/mind-comic\/([^" '\)]+\.(?:webp|woff2))/g)].map(m=>m[1]));
const source=walk('src').filter(f=>/\.(tsx?|jsx?|css)$/.test(f)&&!f.endsWith('comicMarkup.js')).map(f=>readFileSync(f,'utf8')).join('\n');
const files=walk(base).filter(f=>/\.(webp|woff2)$/.test(f)).map(f=>relative(base,f).replaceAll('\\','/'));
for(const file of files){
 const stem=file.replace(/\.(webp|woff2)$/,'');
 if(source.includes(file)||source.includes(`'${stem}'`)||source.includes(`"${stem}"`))required.add(file);
}
const missing=[...required].filter(f=>!files.includes(f));
const unused=files.filter(f=>!required.has(f));
const result={assets:[...required].sort(),missing,unused,unusedBytes:unused.reduce((n,f)=>n+statSync(join(base,f)).size,0)};
if(process.argv.includes('--dist')) {
 const built=walk('dist/mind-comic').map(f=>relative('dist/mind-comic',f).replaceAll('\\','/')).sort();
 result.buildMissing=result.assets.filter(f=>!built.includes(f));
 result.buildUnexpected=built.filter(f=>!required.has(f));
 if(result.buildMissing.length||result.buildUnexpected.length)process.exitCode=1;
}
if(process.argv.includes('--write'))writeFileSync(join(base,'assets.json'),JSON.stringify({assets:result.assets},null,2)+'\n');
console.log(JSON.stringify(result,null,2));
if(missing.length)process.exitCode=1;
