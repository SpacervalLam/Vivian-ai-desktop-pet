pub mod utils { pub mod fs {
 pub fn write_atomic(path:&std::path::Path,text:&str)->std::io::Result<()> {if let Some(p)=path.parent(){std::fs::create_dir_all(p)?;}std::fs::write(path,text)}
}
pub mod path {
    pub fn get_user_data_dir() -> std::path::PathBuf {
        static ROOT: once_cell::sync::Lazy<tempfile::TempDir> = once_cell::sync::Lazy::new(||{let p=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/hot-runtime");std::fs::create_dir_all(&p).unwrap();tempfile::tempdir_in(p).unwrap()});
        ROOT.path().to_path_buf()
    }
}}
#[path = "../../src/character_packages.rs"]
pub mod character_packages;

// Only the surrounding application types are stubbed; exercise the real mood renderer.
pub mod psychology {
    #[derive(Clone, Copy)]
    pub enum EmotionLabel {Joy,Closeness,Sadness,Loneliness,Fear,Anger,Curiosity}
}
pub mod schemas {
    #[derive(Debug, Clone, serde::Deserialize)]
    pub struct CharacterExpression {
        pub tsundere:f64,pub clingy:f64,pub genki:f64,pub sass:f64,
        pub healing:f64,pub curiosity:f64,pub ritual:f64,pub habit_awareness:f64,
    }
    pub struct Persona {pub expression:CharacterExpression}
    pub fn default_persona_for(_id:&str)->Persona {
        Persona {expression:CharacterExpression {tsundere:0.3,clingy:0.5,genki:0.75,sass:0.65,healing:0.65,curiosity:0.75,ritual:0.5,habit_awareness:0.65}}
    }
}
#[path = "../../src/persona/mood_expression.rs"]
pub mod mood_expression;

#[path = "../../src/character_residency.rs"]
pub mod character_residency;

pub mod proactive {pub mod triggers {#[derive(Clone,Copy)]pub enum ProactiveTrigger {MoodDriven,Icebreaker,WelcomeBack,Spontaneous,HourlyGreeting,IdleGreeting,HealthReminder,WindowTrigger,TopicExtension,MemoryRecall,TeasingResponse,CrossCharacterReply,BystanderInterjection,Sunrise,Sunset,SystemPressure,ScreenPeek,AppDuration,LateNight,MusicChanged,WorkNotice}}}
#[path="../../src/character_behavior.rs"]pub mod character_behavior;
