use std::{hint::black_box, time::Instant};
use vivian_memory_access_tests::memory::{types::{Granularity,MemoryItem},kinds,retriever::bm25_search};

fn main() {
    for n in [10_000,50_000,200_000] {
        let mut entries:Vec<_>=(0..n).map(|i| {
            let mut item=MemoryItem::new(format!("用户第 {i} 次讨论项目{}：使用 Rust 与 SQLite 整理资料，确认阅读进度、测试记录和后续工作。{}",i%100,"保留原始证据以及不同话题的边界。".repeat(3)),Granularity::Summary,0.7);
            item.id=format!("scale-{i}");item.memory_type="user".into();
            kinds::initialize_record(&mut item);item
        }).collect();
        for (offset,text) in ["用户现在住在上海浦东", "用户明确说不吃香菜", "用户的小猫名字叫团子", "用户下个月准备报名考研"].iter().enumerate() {
            entries[offset*101].content=text.to_string();
        }
        let queries=["上海浦东住址","香菜饮食忌口","小猫团子的名字","考研报名计划"];
        if !std::env::args().any(|arg|arg=="--index-only") {
        for query in queries {black_box(bm25_search(&entries,query,24));}
        let mut times=Vec::new();let mut correct=0;
        for sample in 0..21 {
            let q=sample%queries.len();let start=Instant::now();
            let hits=bm25_search(&entries,queries[q],24);
            times.push(start.elapsed().as_micros());
            correct+=usize::from(hits.iter().take(3).any(|(id,_)|id==&format!("scale-{}",q*101)));
            black_box(hits);
        }
        times.sort();
        println!("baseline n={n} p50_us={} p95_us={} hit_at_3={correct}/21 candidate_body_bytes={}",times[10],times[19],entries.iter().map(|m|m.content.len()).sum::<usize>());
        }
        let dir=tempfile::tempdir().unwrap();
        let store=std::sync::Arc::new(vivian_memory_access_tests::memory::entry_store::MemoryEntryStore::open(dir.path().join("entries.db")).unwrap());
        let start=Instant::now();
        for batch in entries.chunks(1000) {store.write_rows(batch.iter().map(|m|(m.id.clone(),m.clone())).collect(),&[]).unwrap();}
        let index_ms=start.elapsed().as_millis();
        let manager=vivian_memory_access_tests::memory::manager::IndexedHarness::new(store.clone(),"vivian");
        let mut times=Vec::new();let mut correct=0;let mut max_candidates=0;let mut max_bytes=0;
        for query in queries {black_box(manager.search(query,None).unwrap());}
        for sample in 0..21 {
            let q=sample%queries.len();let start=Instant::now();
            let (hits,count,bytes)=manager.search(queries[q],None).unwrap();
            times.push(start.elapsed().as_micros());
            correct+=usize::from(hits.iter().take(3).any(|hit|hit.id==format!("scale-{}",q*101)));
            max_candidates=max_candidates.max(count);max_bytes=max_bytes.max(bytes);
            black_box(hits);
        }
        times.sort();
        assert_eq!(correct,21,"indexed recall quality regression");
        assert!(max_candidates<=384,"candidate budget exceeded");
        let mut broad=Vec::new();let mut broad_count=0;let mut broad_bytes=0;
        for _ in 0..7 {let start=Instant::now();let (hits,count,bytes)=manager.search("Rust 项目",None).unwrap();broad.push(start.elapsed().as_micros());broad_count=count;broad_bytes=bytes;black_box(hits);}
        broad.sort();
        println!("indexed n={n} p50_us={} p95_us={} hit_at_3={correct}/21 max_candidates={max_candidates} candidate_body_bytes={max_bytes} index_build_ms={index_ms} broad_p50_us={} broad_candidates={broad_count} broad_body_bytes={broad_bytes}",times[10],times[19],broad[3]);
    }
}
