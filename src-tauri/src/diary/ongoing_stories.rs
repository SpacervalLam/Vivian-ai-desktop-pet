use crate::error::VivianResult;
use serde::{Deserialize, Serialize};
use std::fs;

use super::StoryUpdate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OngoingStory {
    pub id: String,
    pub title: String,
    pub started_date: String,
    pub last_updated: String,
    pub status: String,
    pub summary: String,
}

fn stories_file(char_id: &str) -> std::path::PathBuf {
    super::diary_dir(char_id).join("ongoing_stories.json")
}

pub fn load_stories(char_id: &str) -> Vec<OngoingStory> {
    let path = stories_file(char_id);
    crate::utils::fs::load_json_or_backup(&path).unwrap_or_default()
}

pub fn active_stories(char_id: &str, limit: usize) -> Vec<OngoingStory> {
    load_stories(char_id)
        .into_iter()
        .filter(|s| s.status == "active")
        .take(limit)
        .collect()
}

pub fn update_ongoing_stories(
    char_id: &str,
    update: &StoryUpdate,
    date: &str,
) -> VivianResult<()> {
    if update.title.is_empty() {
        return Ok(());
    }

    let path = stories_file(char_id);
    let mut stories = load_stories(char_id);

    if let Some(existing) = stories
        .iter_mut()
        .find(|s| s.title == update.title && s.status == "active")
    {
        existing.last_updated = date.to_string();
        if !update.status.is_empty() {
            existing.status = update.status.clone();
        }
        if !update.summary.is_empty() {
            existing.summary = update.summary.clone();
        }
    } else if update.status == "active"
        && stories.iter().filter(|s| s.status == "active").count() < 5
    {
        stories.push(OngoingStory {
            id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
            title: update.title.clone(),
            started_date: date.to_string(),
            last_updated: date.to_string(),
            status: "active".to_string(),
            summary: update.summary.clone(),
        });
    }

    prune_resolved(&mut stories);

    let json = serde_json::to_string_pretty(&stories)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn prune_resolved(stories: &mut Vec<OngoingStory>) {
    let now = chrono::Local::now().date_naive();
    stories.retain(|s| {
        if s.status != "resolved" {
            return true;
        }
        chrono::NaiveDate::parse_from_str(&s.last_updated, "%Y-%m-%d")
            .map(|d| (now - d).num_days() < 7)
            .unwrap_or(true)
    });
}
