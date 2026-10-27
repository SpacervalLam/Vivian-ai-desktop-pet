// Production retrieval modules with deterministic local vector/embedding doubles.
// Uses isolated SQLite paths; never loads application data or contacts a provider.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dir = path.join(root, 'tmp/memory-access-tests');
const rustPath = file => path.join(root, file).replaceAll('\\', '/');
const readSource = async file => {
    if (!process.argv.includes('--baseline')) return readFile(path.join(root,file),'utf8');
    const result=spawnSync('git',['show',`HEAD:${file}`],{cwd:root,encoding:'utf8'});
    if(result.status!==0) throw new Error(result.stderr);
    return result.stdout;
};
await mkdir(path.join(dir, 'src'), { recursive: true });
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-memory-access-tests"
version="0.1.0"
edition="2021"
[dependencies]
vivian-storage-contracts={path="${rustPath('src-tauri/storage-contracts')}"}
serde={version="1",features=["derive"]}
serde_json="1"
sha2="0.10"
parking_lot="0.12"
once_cell="1"
chrono="0.4"
uuid={version="1",features=["v4"]}
tracing="0.1"
jieba-rs="0.7"
regex="1"
rusqlite={version="0.32",features=["bundled","backup"]}
lru="0.12"
tempfile="3"
tokio={version="1",features=["rt-multi-thread","macros","time","sync"]}
`);
const retention = await readFile(path.join(root, 'src-tauri/src/memory/retention.rs'), 'utf8');
const decay = retention.slice(retention.indexOf('pub struct MemoryRetentionPolicy'), retention.indexOf('/// 记忆保留守卫'));
const embedding = await readFile(path.join(root, 'src-tauri/src/memory/embedding.rs'), 'utf8');
const hashing = embedding.slice(embedding.indexOf('pub struct HashingMemoryEmbedding'), embedding.indexOf('pub(crate) fn can_retry_embedding_individually'));
const manager = await readSource('src-tauri/src/memory/manager.rs');
const summaries = await readFile(path.join(root, 'src-tauri/src/memory/session_summary.rs'), 'utf8');
const innerStruct = manager.slice(manager.indexOf('struct MemoryManagerInner {'), manager.indexOf('\n}',manager.indexOf('struct MemoryManagerInner {'))+2);
const innerImpl = manager.slice(manager.indexOf('impl MemoryManagerInner {'), manager.indexOf('    fn seed_if_empty(',manager.indexOf('impl MemoryManagerInner {'))) + '\n}';
const summarizedPart=manager.slice(manager.indexOf('pub fn is_summarized('),manager.indexOf('fn apply_observer_penalty('));
const summarized=summarizedPart.slice(0,summarizedPart.lastIndexOf('}')+1);
const incremental=innerStruct.includes('changes:');
const indexed=manager.includes('let lexical = store.search_candidates(');
function rustItem(source, marker) {
    const start=source.indexOf(marker);if(start<0)throw new Error(`Missing Rust item: ${marker}`);
    const open=source.indexOf('{',start);let depth=1;let i=open+1;
    for(;i<source.length && depth;i++){if(source[i]==='{')depth++;else if(source[i]==='}')depth--;}
    return source.slice(start,i);
}
const indexedManager = indexed ? `
${rustItem(manager,'struct RecallSnapshot {')}
${rustItem(manager,'impl RecallSnapshot {')}
pub struct IndexedHarness { inner:Arc<parking_lot::RwLock<MemoryManagerInner>>,knowledge_graph:Arc<super::graph_store::KnowledgeGraph>,char_id:String,_dir:tempfile::TempDir }
impl IndexedHarness {
    pub fn char_id(&self)->&str{&self.char_id}
    ${manager.includes('pub fn consolidation_summaries(') ? rustItem(manager,'pub fn consolidation_summaries(') : ''}
    ${manager.includes('pub fn count_diary_material(') ? rustItem(manager,'pub fn count_diary_material(') : ''}
    ${rustItem(manager,'fn recall_snapshot(')}
    pub fn new(store:Arc<super::entry_store::MemoryEntryStore>, owner:&str)->Self {
        let dir=tempfile::tempdir().unwrap();
        let inner=MemoryManagerInner{data:MemoryStoreData::default(),id_index:HashMap::new(),capacities:HashMap::new(),entry_store:store,changes:Default::default(),revision:0,
            vector_store:Arc::new(MemoryVectorStore::new(dir.path().join("vectors.db"),256,"hashing").unwrap()),embedding:Arc::new(super::embedding::HashingMemoryEmbedding::default()),retrieval_weights:RetrievalWeights::default(),dirty:false,last_save_at:None};
        Self{inner:Arc::new(parking_lot::RwLock::new(inner)),knowledge_graph:Arc::new(super::graph_store::KnowledgeGraph::new(dir.path().join("graph.db"))),char_id:owner.into(),_dir:dir}
    }
    pub fn search(&self,query:&str,filter:Option<&MemoryRetrievalFilter>)->VivianResult<(Vec<MemoryItem>,usize,usize)>{
        let snapshot=self.recall_snapshot(query,RetrievalStrategy::Auto,filter,24,None)?;
        let count=snapshot.entries.len();let bytes=snapshot.entries.iter().map(|item|item.content.len()).sum();
        Ok((snapshot.search(query,RetrievalStrategy::Auto,24,None,&self.knowledge_graph),count,bytes))
    }
}
#[cfg(test)] mod indexed_integration {
    use super::*;
    ${manager.includes('pub fn count_diary_material(') ? `
    #[test] fn diary_material_counter_is_scoped_deduplicated_and_capped() {
        let dir=tempfile::tempdir().unwrap();let store=Arc::new(super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
        let manager=IndexedHarness::new(store,"vivian");
        let make=|id:&str|{let mut item=MemoryItem::new("原始对话".into(),Granularity::Turn,0.7);item.id=id.into();item.timestamp=10.0;
            item.memory_type="short_term".into();item.metadata=serde_json::json!({"channel":"direct","speaker":"user","listener":"vivian"});super::super::kinds::initialize_record(&mut item);item};
        let mut original=make("one");original.metadata["utterance_id"]=serde_json::json!("same");let mut duplicate=original.clone();duplicate.id="copy".into();
        let second=make("two");let mut fact=make("fact");fact.metadata["record_kind"]=serde_json::json!("fact");
        let mut seed=make("seed");seed.metadata["source"]=serde_json::json!("system_seed");
        let mut private=make("other-role");private.metadata["known_by"]=serde_json::json!(["nana"]);
        let mut cross=make("peer");cross.metadata["channel"]=serde_json::json!("cross_character");
        let mut observed=make("observed");observed.metadata["speaker"]=serde_json::json!("nana");observed.metadata["listener"]=serde_json::json!("vivian");
        let mut empty=make("empty");empty.content.clear();let mut past=make("past");past.timestamp=-10.0;
        manager.inner.write().data.entries=vec![original,duplicate,second,fact,seed,private,cross,observed,empty,past];
        assert_eq!(manager.count_diary_material(0.0,20.0,20),2);assert_eq!(manager.count_diary_material(0.0,20.0,1),1);
        assert_eq!(manager.count_diary_material(0.0,20.0,0),0);assert_eq!(manager.count_diary_material(20.0,30.0,20),0);
    }
    ` : ''}
    ${manager.includes('pub fn consolidation_summaries(') ? `
    #[test] fn consolidation_recovers_cold_empty_checkpoints_and_respects_resident_retirement() {
        let dir=tempfile::tempdir().unwrap();let store=Arc::new(super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
        let make=|conversation:&str|{
            let mut item=MemoryItem::new(String::new(),Granularity::Summary,0.1);
            item.id=super::super::session_summary::summary_id("vivian",conversation);item.memory_type="session_summary".into();
            item.metadata=serde_json::json!({"conversation_id":conversation,"source":"session_consolidation","summary_status":"no_content","summary_parts":[]});item
        };
        let empty=make("cold-empty"); let stale=make("resident-stale"); let mut retired=make("cold-retired");retired.consolidated=true;
        store.write_rows(vec![(empty.id.clone(),empty.clone()),(stale.id.clone(),stale.clone()),(retired.id.clone(),retired)],&[]).unwrap();
        let manager=IndexedHarness::new(store,"vivian");
        {let mut inner=manager.inner.write();let mut stale=stale;stale.metadata["index_active"]=serde_json::json!(false);inner.data.entries.push(stale);inner.rebuild_index();}
        let ids=vec!["cold-empty".into(),"resident-stale".into(),"cold-retired".into(),"absent".into(),"cold-empty".into()];
        let result=manager.consolidation_summaries(&ids).unwrap();
        assert_eq!(result.len(),1);assert_eq!(result[0].id,empty.id);assert!(result[0].content.is_empty());
        assert_eq!(manager.inner.read().data.entries.len(),1,"cold checkpoint reads must not refill the resident cache");
    }
    ` : ''}
    #[test] fn indexed_recall_reads_cold_records_and_filters_before_ranking() {
        let dir=tempfile::tempdir().unwrap();let store=Arc::new(super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
        let mut entries:Vec<_>=(0..2_000).map(|i|{let mut row=MemoryItem::new(format!("项目{i}使用 Rust 开发"),Granularity::Summary,0.7);row.memory_type="user".into();row.metadata=serde_json::json!({"known_by":["nana"]});super::super::kinds::initialize_record(&mut row);row}).collect();
        let mut fact=MemoryItem::new("用户的小猫名字叫团子".into(),Granularity::Summary,0.8);fact.id="cold-pet".into();fact.memory_type="user".into();super::super::kinds::initialize_record(&mut fact);entries.push(fact);
        store.write_rows(entries.iter().map(|row|(row.id.clone(),row.clone())).collect(),&[]).unwrap();
        let manager=IndexedHarness::new(store,"vivian");
        let (hits,count,_)=manager.search("小猫团子的名字",None).unwrap();assert!(hits.iter().any(|row|row.id=="cold-pet"));assert!(count<=384);
        assert!(manager.search("Rust",None).unwrap().0.is_empty());
        let filter=MemoryRetrievalFilter{memory_types:Some(vec!["knowledge".into()]),..Default::default()};assert!(manager.search("小猫",Some(&filter)).unwrap().0.is_empty());
        assert!(manager.inner.read().data.entries.is_empty(),"cold recall must not populate or scan the resident collection");
    }
    #[test] fn authored_seed_queries_keep_baseline_recall_quality() {
        let source=include_str!("${rustPath('src-tauri/prompts/characters/vivian/seed_memories.md')}").replace("\\r\\n","\\n");
        let raw=if source.trim_start().starts_with('{'){source.trim()}else{source.split_once("\x60\x60\x60json\\n").unwrap().1.split_once("\\n\x60\x60\x60").unwrap().0};
        let value:serde_json::Value=serde_json::from_str(raw).unwrap();
        let entries:Vec<MemoryItem>=serde_json::from_value(value["memories"].clone()).unwrap();
        let cases:[(&str,&[&str]);8]=[
            ("你们认识AlenTinn吗？", &["名字先留下来"]),
            ("第一次能上网的时候你在找什么", &["搜索框一开就没停下来"]),
            ("你为什么想要类似Codex的功能", &["我也想要那个能改文件的"]),
            ("办公智能体第一次帮你做了什么", &["办公页里真的多了一个文件", "那个小文件没有删"]),
            ("你之前要的B站功能后来有了吗", &["我之前要的 B 站呢"]),
            ("窗口重力是怎么试的", &["能不能让我坐在窗口上", "那你把窗口再移一下"]),
            ("你为什么想要漫画房间", &["我想要画出来的那个房间"]),
            ("网页终于可以在应用里面打开了", &["这次网页留在这里了"]),
        ];
        let dir=tempfile::tempdir().unwrap();let store=Arc::new(super::super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
        store.write_rows(entries.iter().map(|row|(row.id.clone(),row.clone())).collect(),&[]).unwrap();
        let manager=IndexedHarness::new(store,"vivian");let embedding:Arc<dyn MemoryEmbeddingProvider>=Arc::new(super::super::embedding::HashingMemoryEmbedding::default());
        let vectors=MemoryVectorStore::new(dir.path().join("baseline.db"),256,"hashing").unwrap();
        let context=RetrievalContext{entries:&entries,vector_store:&vectors,embedding:&embedding,graph:None,weights:None,query_embedding:QueryEmbedding::Prepared(None),prepared_arms:None};
        let mut baseline_hit=0;let mut indexed_hit=0;let mut baseline_mrr=0.0;let mut indexed_mrr=0.0;
        for (query,expected) in cases {
            let baseline=create_strategy(&RetrievalStrategy::Auto).search(&context,query,24);
            let (indexed,_,_)=manager.search(query,None).unwrap();
            let rank=|items:&[MemoryItem]|items.iter().position(|item|expected.iter().any(|description|item.description.as_deref()==Some(description)));
            if let Some(rank)=rank(&baseline){baseline_hit+=usize::from(rank<3);baseline_mrr+=1.0/(rank+1) as f64;}
            if let Some(rank)=rank(&indexed){indexed_hit+=usize::from(rank<3);indexed_mrr+=1.0/(rank+1) as f64;}
        }
        eprintln!("authored recall baseline_hit3={baseline_hit}/8 indexed_hit3={indexed_hit}/8 baseline_mrr={} indexed_mrr={}",baseline_mrr/8.0,indexed_mrr/8.0);
        assert!(indexed_hit>=baseline_hit);assert!(indexed_mrr+1e-9>=baseline_mrr);
    }
}
` : '';
const dedupPart = manager.slice(manager.indexOf('pub fn dedup_by_content('), manager.indexOf('pub fn is_observer_perspective('));
const dedup = dedupPart.slice(0,dedupPart.lastIndexOf('}')+1);
const retriever = await readSource('src-tauri/src/memory/retriever.rs');
await writeFile(path.join(dir, 'src/retriever.rs'), retriever.replace('fn bm25_search(', 'pub fn bm25_search('));
await writeFile(path.join(dir, 'src/strategy.rs'), await readSource('src-tauri/src/memory/strategy.rs'));
await writeFile(path.join(dir, 'src/lib.rs'), String.raw`
#![allow(dead_code, unused_mut)]
pub use vivian_storage_contracts::{error,utils};
pub mod config { pub mod manager {
    pub struct RetrievalWeightsConfig { pub recency:f64,pub relevance:f64,pub importance:f64,pub hook_boost:f64,pub need_sim:f64,pub recency_tau_hours:f64,pub min_score:f64 }
} }
pub mod memory {
    pub use vivian_storage_contracts::{types,entry_store,search_index,provenance};
    pub mod session_summary { __SUMMARY_HELPERS__ }
    pub use embedding::MemoryEmbeddingProvider;
    __MODULES__
    pub mod retriever { include!(concat!(env!("CARGO_MANIFEST_DIR"),"/src/retriever.rs")); }
    pub mod retention { use super::types::MemoryItem; use std::collections::HashSet; __DECAY__ }
    pub mod manager {
        use super::{types::{MemoryItem,MemoryStoreData,Granularity,RetrievalStrategy,MemoryRetrievalFilter},vector_search::MemoryVectorStore,embedding::MemoryEmbeddingProvider,retriever::RetrievalWeights,graph_store::KnowledgeGraph,strategy::{create_strategy,QueryEmbedding,RetrievalContext}};
        use crate::error::VivianResult;
        use std::{collections::HashMap,sync::Arc,str::FromStr};
        const MAX_RESIDENT_ENTRIES:usize=20_000;
        const RESIDENT_EVICT_TARGET:usize=18_000;
        __DEDUP__
        __SUMMARIZED__
        __INNER_STRUCT__
        __INNER_IMPL__
        __ACCESS_TESTS__
        __INDEXED_MANAGER__
        pub fn benchmark_save(n:usize)->u128 {
            let dir=tempfile::tempdir().unwrap();
            let store=super::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap();
            let entries:Vec<_>=(0..n).map(|i|{let mut item=MemoryItem::new(format!("项目 {i} 使用 Rust 和 SQLite，保留原话证据与约定。"),Granularity::Summary,0.8);item.id=format!("row-{i}");item}).collect();
            store.write_rows(entries.iter().map(|m|(m.id.clone(),m.clone())).collect(),&[]).unwrap();
            let mut inner=MemoryManagerInner {
                id_index:entries.iter().enumerate().map(|(i,m)|(m.id.clone(),i)).collect(),
                __TRACKER_INIT__
                data:MemoryStoreData{version:4,entries},capacities:HashMap::new(),entry_store:__STORE_INIT__,
                vector_store:__VECTOR_INIT__,embedding:Arc::new(super::embedding::HashingMemoryEmbedding::default()),
                retrieval_weights:RetrievalWeights::default(),dirty:false,last_save_at:None,
            };
            let mut times=Vec::new();
            for i in 0..7 { __MUTATE_ONE__
                let start=std::time::Instant::now();inner.save_to_disk().unwrap();times.push(start.elapsed().as_micros());
            }
            times.sort();times[3]
        }
    }
    pub mod embedding {
        use crate::error::VivianResult;
        use super::tokenize::tokenize;
        fn fnv1a_64(text:&str)->u64 { let mut hash=0xcbf29ce484222325u64; for byte in text.bytes(){hash^=byte as u64;hash=hash.wrapping_mul(0x100000001b3);} hash }
        pub trait MemoryEmbeddingProvider:Send+Sync {
            fn dimension(&self)->usize;
            fn embed(&self,text:&str)->VivianResult<Vec<f32>>;
            fn embed_batch(&self,texts:&[String])->VivianResult<Vec<Vec<f32>>>{texts.iter().map(|text|self.embed(text)).collect()}
            fn model_id(&self)->&str {"test"}
            fn is_remote(&self)->bool {false}
            fn embed_batch_chunked(&self,texts:&[String],_:usize,progress:&(dyn Fn(usize,usize)+Send+Sync))->VivianResult<Vec<Vec<f32>>>{let all=texts.iter().map(|s|self.embed(s)).collect::<VivianResult<Vec<_>>>()?;progress(all.len(),all.len());Ok(all)}
        }
        __HASHING__
    }
    pub mod vector_search {
        use crate::error::VivianResult;
        use parking_lot::Mutex;
        use std::path::PathBuf;
        #[derive(Clone)] pub struct MemoryVector { pub doc_id:String,pub memory_id:String,pub content:String,pub embedding:Vec<f32>,pub importance:f64,pub memory_type:String,pub timestamp:f64 }
        pub struct MemoryVectorStore { rows:Mutex<Vec<MemoryVector>> }
        impl MemoryVectorStore {
            pub fn new(_:PathBuf,_:usize,_:&str)->VivianResult<Self>{Ok(Self{rows:Mutex::new(vec![])})}
            pub fn add(&self,row:MemoryVector)->VivianResult<()>{self.rows.lock().push(row);Ok(())}
            pub fn is_empty(&self)->bool{self.rows.lock().is_empty()}
            pub fn delete_by_memory_id(&self,id:&str)->bool{let mut rows=self.rows.lock();let before=rows.len();rows.retain(|r|r.memory_id!=id);rows.len()!=before}
            pub fn search(&self,q:&[f32],k:usize)->Vec<(String,String,f64)>{let mut hits:Vec<_>=self.rows.lock().iter().map(|r|(r.doc_id.clone(),r.memory_id.clone(),cosine_similarity(q,&r.embedding))).filter(|r|r.2>0.0).collect();hits.sort_by(|a,b|b.2.total_cmp(&a.2));hits.truncate(k);hits}
        }
        pub fn cosine_similarity(a:&[f32],b:&[f32])->f64{if a.len()!=b.len(){return 0.0;}let dot:f64=a.iter().zip(b).map(|(x,y)|*x as f64 * *y as f64).sum();let norm=|v:&[f32]|v.iter().map(|x|(*x as f64).powi(2)).sum::<f64>().sqrt();dot/(norm(a)*norm(b)).max(1e-9)}
    }
}
`.replace('__MODULES__', ['age','evidence','tokenize','entity_extract','graph_store','relational_recall','kinds','query_embedding'].map(name => `#[path="${rustPath(`src-tauri/src/memory/${name}.rs`)}"] pub mod ${name};`).join('\n') + `\n#[path="${path.join(dir,'src/strategy.rs').replaceAll('\\','/')}"] pub mod strategy;`)
  .replace('__SUMMARY_HELPERS__', rustItem(summaries,'pub fn summary_id(')+'\n'+rustItem(summaries,'pub fn is_active_summary('))
  .replace('__DECAY__', decay).replace('__HASHING__', hashing.slice(0,hashing.lastIndexOf('}')+1)).replace('__DEDUP__', dedup)
  .replace('__SUMMARIZED__',summarized).replace('__INNER_STRUCT__',innerStruct).replace('__INNER_IMPL__',innerImpl)
  .replace('__ACCESS_TESTS__',incremental?`#[cfg(test)] #[path="${rustPath('src-tauri/src/memory/manager_access_tests.rs')}"] mod access_tests;`:'')
  .replace('__INDEXED_MANAGER__',indexedManager)
  .replace('__TRACKER_INIT__',incremental?'changes:Default::default(),revision:0,':'persisted:super::entry_store::fingerprint_all(&entries),')
  .replace('__STORE_INIT__',innerStruct.includes('entry_store: Arc<')?'Arc::new(store)':'store')
  .replace('__VECTOR_INIT__',`${incremental?'Arc::new(':''}MemoryVectorStore::new(dir.path().join("vectors.db"),256,"hashing").unwrap()${incremental?')':''}`)
  .replace('__MUTATE_ONE__',incremental?'inner.entry_mut(i).unwrap().visit_count+=1;':'inner.data.entries[i].visit_count+=1;'));
// Include as an ordinary module so production inner documentation stays valid.
const lib = await readFile(path.join(dir, 'src/lib.rs'), 'utf8');
await writeFile(path.join(dir, 'src/lib.rs'), lib.replace('pub mod retriever { include!(concat!(env!("CARGO_MANIFEST_DIR"),"/src/retriever.rs")); }', `#[path="${path.join(dir, 'src/retriever.rs').replaceAll('\\','/')}"] pub mod retriever;`));
if (process.argv.includes('--scale')) {
    await writeFile(path.join(dir, 'src/main.rs'), await readFile(path.join(root,'tests/fixtures/memory-scale-bench.rs'),'utf8'));
} else if (process.argv.includes('--bench')) {
    await writeFile(path.join(dir, 'src/main.rs'), String.raw`
use std::{hint::black_box,time::Instant,collections::HashMap};
use vivian_memory_access_tests::memory::{types::{MemoryItem,Granularity},retriever::bm25_search,entry_store::fingerprint_all};
fn main(){
    for n in [1_000,8_000,8_001,10_000] {
        let entries:Vec<_>=(0..n).map(|i|{let mut m=MemoryItem::new(format!("用户记录 {i}：项目使用 Rust 和 SQLite。优化检索性能时保留原话证据，记住每次讨论的约定与偏好。"),Granularity::Summary,0.8);m.id=format!("bench-{n}-{i}");m.tags=vec!["project".into(),"preference".into()];m}).collect();
        black_box(bm25_search(&entries,"Rust 项目检索",5));
        let mut times=Vec::new();for _ in 0..7{let start=Instant::now();black_box(bm25_search(&entries,"Rust 项目检索",5));times.push(start.elapsed().as_micros());}times.sort();
        let mut writes=Vec::new();for _ in 0..7{let start=Instant::now();black_box(fingerprint_all(&entries));writes.push(start.elapsed().as_micros());}writes.sort();
        let _index:HashMap<_,_>=entries.iter().enumerate().map(|(i,m)|(m.id.clone(),i)).collect();
        println!("n={n} bm25_warm_median_us={} fingerprint_all_median_us={} one_row_save_median_us={}",times[3],writes[3],vivian_memory_access_tests::memory::manager::benchmark_save(n));
    }
}
`);
}
const benchmark = process.argv.includes('--bench') || process.argv.includes('--scale');
const args = benchmark ? ['run','--release','--bin','vivian-memory-access-tests'] : ['test','--lib'];
const result = spawnSync('cargo', [...args,'--offline','--manifest-path',path.join(dir,'Cargo.toml'),'-j','1', ...(benchmark?(process.argv.includes('--index-only')?['--','--index-only']:[]):['--','--test-threads=1'])], {
    cwd:root,stdio:'inherit',env:{...process.env,CARGO_INCREMENTAL:'0'},
});
if(result.error) throw result.error;
process.exitCode=result.status??1;
