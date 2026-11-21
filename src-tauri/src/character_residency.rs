//! Public move-in facts only. Personal histories and private state remain per character.
use std::{collections::{BTreeMap,BTreeSet},path::Path};
use serde::{Deserialize,Serialize};
#[derive(Clone)]
pub struct Resident {pub id:String,pub name:String,pub present:bool}
#[derive(Default,Deserialize,Serialize)]
struct Journal {arrivals:BTreeMap<String,Arrival>}
#[derive(Deserialize,Serialize)]
struct Arrival {name:String,at:i64,witnesses:BTreeSet<String>}
pub struct Notice {pub recipient:String,pub character:String,pub name:String,pub at:i64}
static LOCK:std::sync::Mutex<()>=std::sync::Mutex::new(());
fn read(path:&Path)->Result<Journal,String>{
    match std::fs::read(path){Ok(bytes)=>serde_json::from_slice(&bytes).map_err(|e|format!("入住记录损坏，已保留原文件: {e}")),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(Journal::default()),Err(e)=>Err(e.to_string())}
}
pub fn record(base:&Path,residents:&[Resident],packages:&BTreeSet<String>,now:i64)->Result<Vec<Notice>,String>{
    let _lock=LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let path=base.join("residency.json");let mut journal=read(&path)?;
    let previous=serde_json::to_string(&journal).map_err(|e|e.to_string())?;
    for resident in residents.iter().filter(|r|packages.contains(&r.id)){
        journal.arrivals.entry(resident.id.clone()).or_insert_with(||Arrival{name:resident.name.clone(),at:now,witnesses:BTreeSet::new()});
    }
    let mut notices=Vec::new();
    for (id,event) in &mut journal.arrivals{
        if !residents.iter().any(|r|r.id==*id){continue;}
        for observer in residents.iter().filter(|r|r.present&&r.id!=*id){
            if event.witnesses.insert(observer.id.clone()){notices.push(Notice{recipient:observer.id.clone(),character:id.clone(),name:event.name.clone(),at:event.at});}
        }
    }
    let next=serde_json::to_string(&journal).map_err(|e|e.to_string())?;
    if next!=previous{crate::utils::fs::write_atomic(&path,&next).map_err(|e|e.to_string())?;}
    Ok(notices)
}
pub fn context(base:&Path,recipient:&str)->Option<String>{
    let journal=read(&base.join("residency.json")).ok()?;
    let facts:Vec<_>=journal.arrivals.iter().filter(|(_,e)|e.witnesses.contains(recipient)).map(|(id,e)|serde_json::json!({"id":id,"name":e.name,"moved_in_at":e.at})).collect();
    if facts.is_empty(){return None;}
    Some(format!("[Public move-in notices received] {}\nThese are past residence registrations, not new events on every turn, not proof of current presence or shared private memories. No repeated announcement or forced greeting is required.",serde_json::to_string(&facts).ok()?))
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn notices_are_once_per_present_observer_and_updates_are_not_new_arrivals(){
        let dir=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/residency-tests");std::fs::create_dir_all(&dir).unwrap();let temp=tempfile::tempdir_in(dir).unwrap();
        let mut residents=vec![Resident{id:"vivian".into(),name:"Vivian".into(),present:true},Resident{id:"nana".into(),name:"Nana".into(),present:false},Resident{id:"deepseek".into(),name:"DeepSeek".into(),present:true}];
        let packages=BTreeSet::from(["deepseek".into(),"claude".into()]);
        let first=record(temp.path(),&residents,&packages,100).unwrap();assert_eq!(first.len(),1);assert_eq!(first[0].recipient,"vivian");
        assert!(record(temp.path(),&residents,&packages,200).unwrap().is_empty());
        assert!(context(temp.path(),"nana").is_none());assert!(context(temp.path(),"vivian").unwrap().contains("DeepSeek"));
        residents[1].present=true;let later=record(temp.path(),&residents,&packages,300).unwrap();assert_eq!(later.len(),1);assert_eq!(later[0].at,100);
        residents.push(Resident{id:"claude".into(),name:"Claude".into(),present:true});
        let second=record(temp.path(),&residents,&packages,400).unwrap();assert_eq!(second.iter().filter(|n|n.character=="claude").count(),3);
        assert!(record(temp.path(),&residents,&packages,500).unwrap().is_empty());
    }
}
