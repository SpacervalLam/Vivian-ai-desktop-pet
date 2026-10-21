//! Atomic registry state. Callers hold one lock for names, ownership and indexes.

use std::collections::{BTreeMap, HashMap};

pub struct Registration<T> {
    pub value: T,
    pub owner: Option<String>,
}

pub struct ToolCatalog<T> {
    entries: BTreeMap<String, Registration<T>>,
    aliases: HashMap<String, String>,
    // None means more than one canonical target: fuzzy lookup must fail closed.
    normalized: HashMap<String, Option<String>>,
    revision: u64,
}

impl<T> Default for ToolCatalog<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            aliases: HashMap::new(),
            normalized: HashMap::new(),
            revision: 0,
        }
    }
}

impl<T> ToolCatalog<T> {
    pub fn from_values(values: impl IntoIterator<Item = (String, T)>) -> Self {
        let mut catalog = Self::default();
        catalog.entries = values
            .into_iter()
            .map(|(name, value)| (name, Registration { value, owner: None }))
            .collect();
        catalog.changed();
        catalog
    }

    pub fn register(&mut self, name: String, value: T, owner: Option<String>) {
        self.aliases.remove(&name);
        self.entries.insert(name, Registration { value, owner });
        self.changed();
    }

    pub fn register_alias(&mut self, from: &str, to: &str) -> bool {
        let (from, to) = (from.trim(), to.trim());
        if from.is_empty()
            || to.is_empty()
            || from == to
            || self.entries.contains_key(from)
            || !self.entries.contains_key(to)
        {
            return false;
        }
        if self.aliases.get(from).is_some_and(|target| target == to) {
            return true;
        }
        self.aliases.insert(from.to_string(), to.to_string());
        self.changed();
        true
    }

    pub fn remove(&mut self, name: &str, owner: Option<&str>) -> bool {
        let Some(entry) = self.entries.get(name) else {
            return false;
        };
        if owner.is_some() && entry.owner.as_deref() != owner {
            return false;
        }
        self.entries.remove(name);
        self.aliases.retain(|_, target| target != name);
        self.changed();
        true
    }

    pub fn resolve<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        if self.entries.contains_key(name) {
            return Some(name);
        }
        if let Some(target) = self.aliases.get(name) {
            return Some(target);
        }
        self.normalized.get(&normalize_tool_name(name))?.as_deref()
    }

    pub fn find(&self, name: &str) -> Option<&T> {
        self.entries
            .get(self.resolve(name)?)
            .map(|entry| &entry.value)
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.values().map(|entry| &entry.value)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.aliases.clear();
        self.changed();
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.normalized.clear();
        for (name, target) in self
            .entries
            .keys()
            .map(|name| (name, name))
            .chain(self.aliases.iter())
        {
            self.normalized
                .entry(normalize_tool_name(name))
                .and_modify(|current| {
                    if current.as_ref() != Some(target) {
                        *current = None;
                    }
                })
                .or_insert_with(|| Some(target.clone()));
        }
    }
}

/// Accept separator/case differences without guessing word order.
pub fn normalize_tool_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_removal_and_aliases_share_one_revision() {
        let mut catalog = ToolCatalog::default();
        catalog.register("wallpaper_set".into(), 1, Some("old".into()));
        assert!(catalog.register_alias("set_wallpaper", "wallpaper_set"));
        let revision = catalog.revision();
        catalog.register("wallpaper_set".into(), 2, Some("new".into()));
        assert!(catalog.revision() > revision);
        assert_eq!(catalog.find("SET.WALLPAPER"), Some(&2));
        assert!(!catalog.remove("wallpaper_set", Some("old")));
        assert!(catalog.remove("wallpaper_set", Some("new")));
        assert!(catalog.find("set_wallpaper").is_none());
        assert!(catalog.names().next().is_none());
    }

    #[test]
    fn ambiguous_normalization_never_selects_an_arbitrary_tool() {
        let mut catalog = ToolCatalog::default();
        catalog.register("a_b".into(), 1, None);
        catalog.register("ab".into(), 2, None);
        assert_eq!(catalog.find("a_b"), Some(&1));
        assert_eq!(catalog.find("ab"), Some(&2));
        assert_eq!(catalog.find("A-B"), None);
        catalog.remove("ab", None);
        assert_eq!(catalog.find("A-B"), Some(&1));
    }

    #[test]
    fn aliases_cannot_shadow_tools_or_outlive_targets() {
        let mut catalog = ToolCatalog::default();
        catalog.register("a".into(), 1, None);
        catalog.register("b".into(), 2, None);
        assert!(!catalog.register_alias("a", "b"));
        assert!(!catalog.register_alias("alias", "missing"));
        assert!(catalog.register_alias("alias", "a"));
        catalog.clear();
        assert!(catalog.find("alias").is_none());
    }
}
