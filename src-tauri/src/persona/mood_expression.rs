//! Mood expression is data owned by each imported character package.
use super::schemas::{default_persona_for, CharacterExpression};
use crate::psychology::EmotionLabel;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct MoodGroup {
    delta: BTreeMap<String, f64>,
    phrasing: [String; 3], // zh, ja, en
    inner: [String; 2],
}
#[derive(Deserialize)]
struct MoodProfile {
    groups: BTreeMap<String, MoodGroup>,
    #[serde(default)] quip: bool,
    attention: String,
    event: String,
}
fn profile(id:&str)->Option<&'static MoodProfile>{
 static CACHE:once_cell::sync::Lazy<std::sync::Mutex<BTreeMap<String,&'static MoodProfile>>>=once_cell::sync::Lazy::new(||std::sync::Mutex::new(BTreeMap::new()));
 let raw=crate::character_packages::text(id,"mood.json");if raw.is_empty(){return None;}
 let mut cache=CACHE.lock().unwrap();if let Some(p)=cache.get(raw){return Some(*p);}
 let p=Box::leak(Box::new(serde_json::from_str::<MoodProfile>(raw).ok()?));cache.insert(raw.into(),p);Some(p)
}
fn group(label: EmotionLabel) -> &'static str {
    match label {
        EmotionLabel::Joy | EmotionLabel::Closeness => "positive",
        EmotionLabel::Sadness | EmotionLabel::Loneliness | EmotionLabel::Fear => "low",
        EmotionLabel::Anger => "anger",
        EmotionLabel::Curiosity => "curiosity",
    }
}
fn gain(intensity: f64) -> f64 {
    if intensity.is_finite() { ((intensity-0.30)/0.70).clamp(0.0,1.0) } else { 0.0 }
}
fn adjust(mut base: CharacterExpression, mood: Option<&MoodGroup>, k: f64) -> CharacterExpression {
    if let Some(mood)=mood { for (field, delta) in &mood.delta {
        let target=match field.as_str() {
            "genki"=>&mut base.genki,"healing"=>&mut base.healing,"sass"=>&mut base.sass,
            "curiosity"=>&mut base.curiosity,"ritual"=>&mut base.ritual,"tsundere"=>&mut base.tsundere,
            "clingy"=>&mut base.clingy,"habit_awareness"=>&mut base.habit_awareness,_=>continue,
        };
        if delta.is_finite() { *target=(*target+delta.clamp(-0.20,0.20)*k).clamp(0.0,1.0); }
    }}
    base
}
pub fn effective_expression(id: &str, dominant: EmotionLabel, intensity: f64) -> CharacterExpression {
    adjust(default_persona_for(id).expression,profile(id).and_then(|p|p.groups.get(group(dominant))),gain(intensity))
}
fn directive_for(p: &MoodProfile, base: CharacterExpression, dominant: EmotionLabel, intensity: f64, lang: &str) -> Option<String> {
    let k=gain(intensity);if k<=0.0 {return None;}
    let mood=p.groups.get(group(dominant))?;
    let e=adjust(base.clone(),Some(mood),k);
    let shift=(e.genki-base.genki).abs()+(e.healing-base.healing).abs()+(e.sass-base.sass).abs()
        +(e.curiosity-base.curiosity).abs()+(e.ritual-base.ritual).abs()+(e.tsundere-base.tsundere).abs();
    let pronounced=shift>=0.12;
    let (index,marker,degree,limit)=match lang {
        "en"=>(2,"[Mood and temperament; never repeat]",if pronounced {"Noticeably "} else {"Slightly "},"This only affects delivery; it changes neither factual judgment nor the task."),
        "ja"=>(1,"[気分と性格・復唱禁止]",if pronounced {"今ははっきり"} else {"少し"},"口調だけの話で、事実判断や用件の処理は変えない。"),
        _=>(0,"[心情与性格，不要复述]",if pronounced {"这会儿明显"} else {"稍微"},"这只影响语气，不改变事实判断和任务交付。"),
    };
    if mood.phrasing[index].trim().is_empty(){return None;}
    let mut body=format!("{degree}{}",mood.phrasing[index]);
    if group(dominant)=="anger" && p.quip && e.sass>=0.42 {body.push_str(match lang {"en"=>" One light quip is fine.","ja"=>" 軽い一言なら構わない。",_=>" 允许一句轻吐槽。"});}
    Some(format!("{marker} {body} {limit}"))
}
pub fn directive(id: &str, dominant: EmotionLabel, intensity: f64, lang: &str) -> Option<String> {
    directive_for(profile(id)?,default_persona_for(id).expression,dominant,intensity,lang)
}
fn inner_for(p: &MoodProfile, dominant: EmotionLabel, intensity: f64, topic: Option<&str>, event: &str) -> Option<String> {
    let topic=topic.map(str::trim).filter(|t|!t.is_empty());let has_event=!event.trim().is_empty();
    let mut lines=Vec::new();
    if intensity.is_finite() && intensity>0.30 {
        if let Some(mood)=p.groups.get(group(dominant)) {let line=&mood.inner[usize::from(intensity>=0.6)];if !line.is_empty(){lines.push(line.clone());}}
    }
    if let Some(topic)=topic {let short=topic.chars().take(40).collect::<String>();if !p.attention.is_empty(){lines.push(p.attention.replace("{topic}",&short));}}
    if has_event && !p.event.is_empty(){lines.push(p.event.clone());}
    (!lines.is_empty()).then(||lines.join("；"))
}
pub fn inner_reaction(id: &str, dominant: EmotionLabel, intensity: f64, topic: Option<&str>, event: &str, _lang: &str) -> Option<String> {
    inner_for(profile(id)?,dominant,intensity,topic,event)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(id: &str)->(MoodProfile,CharacterExpression){
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root=if root.join("../character-packs").is_dir(){root.join("../character-packs")}else{root.join("../../character-packs")};
        let read=|file:&str|std::fs::read_to_string(root.join(id).join("prompts").join(file)).unwrap();
        (serde_json::from_str(&read("mood.json")).unwrap(),serde_json::from_str(&read("traits.json")).unwrap())
    }
    #[test]
    fn optional_mood_profiles_preserve_distinct_voices_and_bounded_parameters(){
        for id in ["deepseek","claude","chatgpt"] {
            let(p,base)=fixture(id);
            for label in [EmotionLabel::Joy,EmotionLabel::Closeness,EmotionLabel::Anger,EmotionLabel::Sadness,EmotionLabel::Fear,EmotionLabel::Loneliness,EmotionLabel::Curiosity]{
                for intensity in [0.0,0.2,0.31,0.9,1.0,f64::NAN]{
                    let e=adjust(base.clone(),p.groups.get(group(label)),gain(intensity));
                    assert!([e.genki,e.healing,e.sass,e.curiosity,e.ritual,e.tsundere,e.clingy,e.habit_awareness].iter().all(|v|v.is_finite()&&(0.0..=1.0).contains(v)));
                    if gain(intensity)==0.0{assert_eq!(format!("{e:?}"),format!("{base:?}"));assert!(directive_for(&p,base.clone(),label,intensity,"zh").is_none());}
                }
                for lang in ["zh","ja","en"]{let line=directive_for(&p,base.clone(),label,0.9,lang).unwrap();assert!(!line.chars().any(|c|c.is_ascii_digit()));assert!(line.contains("不要复述")||line.contains("never repeat")||line.contains("復唱禁止"));}
            }
            assert!(inner_for(&p,EmotionLabel::Joy,0.2,None," ").is_none());
            let line=inner_for(&p,EmotionLabel::Anger,0.8,Some("空数组"),"需求变化").unwrap();assert!(line.contains("空数组"));assert!(!line.contains("气死我了"));
        }
        let(ds,db)=fixture("deepseek");let(cl,cb)=fixture("claude");
        assert!(adjust(db.clone(),ds.groups.get("anger"),1.0).sass>db.sass);
        assert!(adjust(cb.clone(),cl.groups.get("anger"),1.0).sass<cb.sass);
        assert_ne!(inner_for(&ds,EmotionLabel::Anger,0.8,None,""),inner_for(&cl,EmotionLabel::Anger,0.8,None,""));
    }
    #[test]
    fn absent_packages_never_inject_another_characters_voice(){
        for id in ["vivian","nana","uninstalled"]{assert!(directive(id,EmotionLabel::Joy,1.0,"zh").is_none());assert!(inner_reaction(id,EmotionLabel::Joy,1.0,None,"event","zh").is_none());}
    }
}
