//! User font/size configuration: parses ~/.config/gitkay/config.toml, resolves
//! named font families to files (cached), and maps the app's text roles to
//! egui FontIds.

use eframe::egui;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MIN_SIZE: f32 = 4.0;
const MAX_SIZE: f32 = 64.0;

/// A piece of UI text whose font/size is configurable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Diff,
    CommitSummary,
    CommitMeta,
    Refs,
    FileList,
    Ui,
}

/// Which configured family a role draws with.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    #[default]
    Monospace,
    Proportional,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct FontsSection {
    pub monospace: Option<String>,
    pub proportional: Option<String>,
    pub monospace_path: Option<String>,
    pub proportional_path: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SizesSection {
    pub diff: f32,
    pub commit_summary: f32,
    pub commit_meta: f32,
    pub refs: f32,
    pub file_list: f32,
    pub ui: f32,
}

impl Default for SizesSection {
    fn default() -> Self {
        Self {
            diff: 13.0,
            commit_summary: 13.0,
            commit_meta: 12.0,
            refs: 11.0,
            file_list: 12.0,
            ui: 13.0,
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct FamiliesSection {
    pub diff: Family,
    pub commit_summary: Family,
    pub commit_meta: Family,
    pub refs: Family,
    pub file_list: Family,
    pub ui: Family,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub fonts: FontsSection,
    pub sizes: SizesSection,
    pub families: FamiliesSection,
}

/// Read + parse the config. A missing file is not an error (returns defaults);
/// a read or parse failure returns `Err(message)` so the caller can decide
/// whether to fall back to defaults (startup) or keep the current look (reload).
pub fn read_config(path: &Path) -> Result<Config, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("cannot read {path:?}: {e}")),
    };
    toml::from_str(&text).map_err(|e| format!("invalid config {path:?}: {e}"))
}

fn egui_family(f: Family) -> egui::FontFamily {
    match f {
        Family::Monospace => egui::FontFamily::Monospace,
        Family::Proportional => egui::FontFamily::Proportional,
    }
}

/// Resolved, clamped font settings ready to hand to egui per role.
pub struct Fonts {
    sizes: SizesSection,
    families: FamiliesSection,
}

impl Fonts {
    pub fn from_config(cfg: &Config) -> Fonts {
        let clamp = |v: f32| v.clamp(MIN_SIZE, MAX_SIZE);
        Fonts {
            sizes: SizesSection {
                diff: clamp(cfg.sizes.diff),
                commit_summary: clamp(cfg.sizes.commit_summary),
                commit_meta: clamp(cfg.sizes.commit_meta),
                refs: clamp(cfg.sizes.refs),
                file_list: clamp(cfg.sizes.file_list),
                ui: clamp(cfg.sizes.ui),
            },
            families: cfg.families.clone(),
        }
    }

    pub fn font_id(&self, role: Role) -> egui::FontId {
        let (size, family) = match role {
            Role::Diff => (self.sizes.diff, self.families.diff),
            Role::CommitSummary => (self.sizes.commit_summary, self.families.commit_summary),
            Role::CommitMeta => (self.sizes.commit_meta, self.families.commit_meta),
            Role::Refs => (self.sizes.refs, self.families.refs),
            Role::FileList => (self.sizes.file_list, self.families.file_list),
            Role::Ui => (self.sizes.ui, self.families.ui),
        };
        egui::FontId::new(size, egui_family(family))
    }

    /// File-list +/- stats: same family as the file list, two px smaller.
    pub fn file_stats_font_id(&self) -> egui::FontId {
        let size = (self.sizes.file_list - 2.0).max(MIN_SIZE);
        egui::FontId::new(size, egui_family(self.families.file_list))
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("gitkay").join("config.toml"))
}

pub fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("gitkay").join("fonts.toml"))
}

/// A fully-commented config showing every default. Written on first run.
pub fn default_template() -> String {
    let s = SizesSection::default();
    format!(
        "# gitkay font configuration — ~/.config/gitkay/config.toml\n\
         # This file is optional. Every line below is commented out and shows its\n\
         # default value. Uncomment and edit to override. Changes apply live on save.\n\
         \n\
         [fonts]\n\
         # Named families, resolved from installed system fonts (cached after the\n\
         # first lookup). Leave unset to use gitkay's bundled fonts.\n\
         # monospace = \"JetBrains Mono\"\n\
         # proportional = \"Inter\"\n\
         # Explicit file paths that skip system-font lookup entirely:\n\
         # monospace_path = \"/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf\"\n\
         # proportional_path = \"/usr/share/fonts/TTF/Inter-Regular.ttf\"\n\
         \n\
         [sizes]\n\
         # diff = {diff}\n\
         # commit_summary = {commit_summary}\n\
         # commit_meta = {commit_meta}   # date / SHA / author\n\
         # refs = {refs}\n\
         # file_list = {file_list}     # filenames; +/- stats render 2px smaller\n\
         # ui = {ui}            # search bar + diff toolbar\n\
         \n\
         [families]\n\
         # Which family each role uses: \"monospace\" or \"proportional\".\n\
         # diff = \"monospace\"\n\
         # commit_summary = \"monospace\"\n\
         # commit_meta = \"monospace\"\n\
         # refs = \"monospace\"\n\
         # file_list = \"monospace\"\n\
         # ui = \"monospace\"\n",
        diff = s.diff,
        commit_summary = s.commit_summary,
        commit_meta = s.commit_meta,
        refs = s.refs,
        file_list = s.file_list,
        ui = s.ui,
    )
}

/// Best-effort write of the default template. Failures (e.g. read-only FS) are
/// silently ignored — the app proceeds on in-memory defaults.
pub fn write_default_template(path: &Path) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, default_template());
}

/// Resolve a configured family name to a font file, using the cache first and
/// the injected `scan` (fontdb) only on a miss or a stale entry.
pub fn resolve_font_path(
    name: &str,
    cache: &mut BTreeMap<String, String>,
    exists: &impl Fn(&Path) -> bool,
    scan: &mut impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(cached) = cache.get(name).cloned() {
        let pb = PathBuf::from(&cached);
        if exists(&pb) {
            return Some(pb);
        }
        cache.remove(name); // stale entry — evict so a failed rescan doesn't leave it behind
    }
    let found = scan(name)?;
    cache.insert(name.to_owned(), found.to_string_lossy().into_owned());
    Some(found)
}

/// Pick a font source for one family: an explicit path wins outright; otherwise
/// resolve the name (cache + scan). Returns `None` when neither is configured.
pub fn family_source(
    name: &Option<String>,
    path: &Option<String>,
    cache: &mut BTreeMap<String, String>,
    exists: &impl Fn(&Path) -> bool,
    scan: &mut impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = path {
        return Some(PathBuf::from(p));
    }
    let name = name.as_ref()?;
    resolve_font_path(name, cache, exists, scan)
}

pub fn load_cache(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_cache(path: &Path, cache: &BTreeMap<String, String>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(s) = toml::to_string(cache) {
        let _ = std::fs::write(path, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_all_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.sizes.diff, 13.0);
        assert_eq!(cfg.sizes.commit_meta, 12.0);
        assert_eq!(cfg.sizes.refs, 11.0);
        assert_eq!(cfg.families.diff, Family::Monospace);
        assert_eq!(cfg.fonts.monospace, None);
    }

    #[test]
    fn partial_sizes_override_only_named_keys() {
        let cfg: Config = toml::from_str("[sizes]\ndiff = 20\n").unwrap();
        assert_eq!(cfg.sizes.diff, 20.0);
        assert_eq!(cfg.sizes.ui, 13.0); // untouched -> default
    }

    #[test]
    fn family_string_parses() {
        let cfg: Config = toml::from_str("[families]\ndiff = \"proportional\"\n").unwrap();
        assert_eq!(cfg.families.diff, Family::Proportional);
        assert_eq!(cfg.families.ui, Family::Monospace);
    }

    #[test]
    fn font_names_parse() {
        let cfg: Config =
            toml::from_str("[fonts]\nmonospace = \"JetBrains Mono\"\n").unwrap();
        assert_eq!(cfg.fonts.monospace.as_deref(), Some("JetBrains Mono"));
    }

    #[test]
    fn missing_file_returns_defaults() {
        let p = std::env::temp_dir().join("gitkay_nonexistent_config_check.toml");
        let _ = std::fs::remove_file(&p); // ensure it does not exist
        assert_eq!(read_config(&p).unwrap(), Config::default());
    }

    #[test]
    fn default_font_ids_match_current_sizes() {
        let fonts = Fonts::from_config(&Config::default());
        assert_eq!(fonts.font_id(Role::Diff).size, 13.0);
        assert_eq!(fonts.font_id(Role::CommitMeta).size, 12.0);
        assert_eq!(fonts.font_id(Role::Refs).size, 11.0);
        assert_eq!(fonts.font_id(Role::Ui).family, egui::FontFamily::Monospace);
    }

    #[test]
    fn proportional_role_uses_proportional_family() {
        let mut cfg = Config::default();
        cfg.families.commit_summary = Family::Proportional;
        let fonts = Fonts::from_config(&cfg);
        assert_eq!(
            fonts.font_id(Role::CommitSummary).family,
            egui::FontFamily::Proportional
        );
    }

    #[test]
    fn sizes_are_clamped() {
        let mut cfg = Config::default();
        cfg.sizes.diff = 1000.0;
        cfg.sizes.refs = 0.5;
        let fonts = Fonts::from_config(&cfg);
        assert_eq!(fonts.font_id(Role::Diff).size, 64.0);
        assert_eq!(fonts.font_id(Role::Refs).size, 4.0);
    }

    #[test]
    fn file_stats_is_two_smaller_than_file_list() {
        let fonts = Fonts::from_config(&Config::default());
        assert_eq!(fonts.font_id(Role::FileList).size, 12.0);
        assert_eq!(fonts.file_stats_font_id().size, 10.0);
    }

    #[test]
    fn template_is_valid_toml_yielding_defaults() {
        // All values are commented out, so parsing yields the defaults.
        let cfg: Config = toml::from_str(&default_template()).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn template_documents_real_default_values() {
        let t = default_template();
        assert!(t.contains("# diff = 13"));
        assert!(t.contains("# commit_meta = 12"));
        assert!(t.contains("# refs = 11"));
        assert!(t.contains("# ui = 13"));
    }

    #[test]
    fn cache_hit_skips_scan() {
        let mut cache = BTreeMap::new();
        cache.insert("Fira".to_owned(), "/fonts/fira.ttf".to_owned());
        let exists = |_: &Path| true;
        let mut scanned = false;
        let mut scan = |_: &str| {
            scanned = true;
            None
        };
        let got = resolve_font_path("Fira", &mut cache, &exists, &mut scan);
        assert_eq!(got, Some(PathBuf::from("/fonts/fira.ttf")));
        assert!(!scanned, "scan must not run on a valid cache hit");
    }

    #[test]
    fn stale_cache_entry_triggers_rescan() {
        let mut cache = BTreeMap::new();
        cache.insert("Fira".to_owned(), "/gone.ttf".to_owned());
        let exists = |_: &Path| false; // cached path no longer exists
        let mut scan = |_: &str| Some(PathBuf::from("/fonts/fira-new.ttf"));
        let got = resolve_font_path("Fira", &mut cache, &exists, &mut scan);
        assert_eq!(got, Some(PathBuf::from("/fonts/fira-new.ttf")));
        assert_eq!(cache.get("Fira").map(String::as_str), Some("/fonts/fira-new.ttf"));
    }

    #[test]
    fn miss_scans_and_caches() {
        let mut cache = BTreeMap::new();
        let exists = |_: &Path| true;
        let mut scan = |_: &str| Some(PathBuf::from("/fonts/found.ttf"));
        let got = resolve_font_path("Inter", &mut cache, &exists, &mut scan);
        assert_eq!(got, Some(PathBuf::from("/fonts/found.ttf")));
        assert_eq!(cache.get("Inter").map(String::as_str), Some("/fonts/found.ttf"));
    }

    #[test]
    fn miss_with_no_match_returns_none() {
        let mut cache = BTreeMap::new();
        let exists = |_: &Path| true;
        let mut scan = |_: &str| None;
        assert_eq!(resolve_font_path("Nope", &mut cache, &exists, &mut scan), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn family_source_prefers_explicit_path() {
        let mut cache = BTreeMap::new();
        let exists = |_: &Path| true;
        let mut scan = |_: &str| -> Option<PathBuf> { panic!("must not scan when path is set") };
        let got = family_source(
            &Some("Whatever".to_owned()),
            &Some("/explicit.ttf".to_owned()),
            &mut cache,
            &exists,
            &mut scan,
        );
        assert_eq!(got, Some(PathBuf::from("/explicit.ttf")));
    }

    #[test]
    fn stale_entry_evicted_when_rescan_fails() {
        let mut cache = BTreeMap::new();
        cache.insert("Fira".to_owned(), "/gone.ttf".to_owned());
        let exists = |_: &Path| false;
        let mut scan = |_: &str| None;
        assert_eq!(resolve_font_path("Fira", &mut cache, &exists, &mut scan), None);
        assert!(!cache.contains_key("Fira"), "stale entry should be evicted");
    }
}
