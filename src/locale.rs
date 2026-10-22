//! Textos do jogo por idioma.
//!
//! Ficheiros `locale/<lang>.json` (mapa chave → string). Uma string `@chave`
//! resolve; sem entrada, fica a string original para o jogo em PT não partir.
//! `VIBER_LANG` escolhe o idioma (default `pt`).

use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;

#[derive(Resource, Clone, Debug, Default)]
pub struct Locale {
    pub lang: String,
    pub table: HashMap<String, String>,
    /// Pasta dos `locale/<lang>.json` (para a troca em runtime).
    pub dir: std::path::PathBuf,
}

impl Locale {
    pub fn resolve(&self, text: &str) -> String {
        crate::dialogue::localized(text, Some(&self.table))
    }

    pub fn load(dir: &Path, lang: &str) -> Self {
        let path = dir.join(format!("{lang}.json"));
        let table = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
            .unwrap_or_default();
        Self {
            lang: lang.to_string(),
            table,
            dir: dir.to_path_buf(),
        }
    }

    /// Troca o idioma sem reiniciar. Um ficheiro ausente deixa a tabela
    /// vazia (as strings originais ficam) — o jogo não parte.
    pub fn set_lang(&mut self, lang: &str) {
        if lang.is_empty() || lang == self.lang {
            return;
        }
        *self = Self::load(&self.dir, lang);
    }
}

pub fn lang_from_env() -> String {
    std::env::var("VIBER_LANG").unwrap_or_else(|_| "pt".into())
}

pub struct LocalePlugin {
    pub dir: std::path::PathBuf,
}

impl Plugin for LocalePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Locale::load(&self.dir, &lang_from_env()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_language_file_is_an_empty_table() {
        let locale = Locale::load(Path::new("/no/such/locale"), "pt");
        assert_eq!(locale.resolve("Olá"), "Olá");
        assert_eq!(locale.resolve("@hi"), "@hi");
    }
}
