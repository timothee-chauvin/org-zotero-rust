use config::{Config, File};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub org_roam_dir: PathBuf,
    pub templates_dir: PathBuf,
    pub zotero_db_path: PathBuf,
}

pub static SETTINGS: Lazy<Settings> = Lazy::new(|| {
    let home_dir = std::env::var("HOME").expect("HOME environment variable not set");
    let config_dir = PathBuf::from(&home_dir).join(".config/org-zotero-rust");
    let config = Config::builder()
        .set_default("config_dir", config_dir.to_string_lossy().to_string())
        .unwrap()
        .add_source(File::with_name(
            &config_dir.join("config.toml").to_string_lossy(),
        ))
        .build()
        .expect("Failed to load configuration from ~/.config/org-zotero-rust/config.toml");

    let mut settings = config.try_deserialize::<Settings>().unwrap();

    // Expand ~ to home directory for all PathBuf fields
    for path in [
        &mut settings.org_roam_dir,
        &mut settings.templates_dir,
        &mut settings.zotero_db_path,
    ] {
        if path.starts_with("~") {
            *path = PathBuf::from(&home_dir).join(path.strip_prefix("~").unwrap());
        }
        if path.is_relative() {
            *path = config_dir.join(path.clone());
        }
    }
    settings
});
