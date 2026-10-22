//! Acesso dos plugins ao projeto, preso à raiz do `config.yaml`.
//!
//! Um plugin é código do projeto, mas continua a ser código: recusamos
//! caminhos absolutos, `..` e symlinks que saiam da raiz. A escrita usa o
//! mesmo `write_atomic` do editor (tmp → `.bak` → rename), por isso um crash
//! nunca deixa um JSON a meio.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Uma entrada de `editor.fs.list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    /// Nome do ficheiro/pasta (sem o caminho).
    pub name: String,
    /// Caminho relativo à raiz do projeto, com `/`.
    pub path: String,
    pub dir: bool,
}

/// O sistema de ficheiros visto por um plugin.
#[derive(Clone, Debug)]
pub struct ProjectFs {
    root: PathBuf,
}

impl ProjectFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = root.canonicalize().unwrap_or(root);
        Self { root }
    }

    /// Junta um caminho relativo, recusando absolutos e `..`.
    fn relative(&self, path: &str) -> Result<PathBuf> {
        let text = path.trim();
        if text.is_empty() {
            bail!("caminho vazio");
        }
        let path = Path::new(text);
        if path.is_absolute() {
            bail!("caminho absoluto não é permitido: {text}");
        }
        for component in path.components() {
            match component {
                Component::Normal(_) => {}
                Component::CurDir => {}
                _ => bail!("caminho fora da raiz do projeto: {text}"),
            }
        }
        Ok(self.root.join(path))
    }

    /// Resolve um caminho que TEM de existir e estar dentro da raiz.
    pub fn resolve_existing(&self, path: &str) -> Result<PathBuf> {
        let joined = self.relative(path)?;
        let canonical = joined
            .canonicalize()
            .with_context(|| format!("não existe: {path}"))?;
        if !canonical.starts_with(&self.root) {
            bail!("caminho fora da raiz do projeto: {path}");
        }
        Ok(canonical)
    }

    /// Resolve um caminho para escrita; a pasta-pai (ou o primeiro ancestral
    /// existente) tem de estar dentro da raiz. As pastas em falta são criadas
    /// no `write`.
    pub fn resolve_for_write(&self, path: &str) -> Result<PathBuf> {
        let joined = self.relative(path)?;
        let mut parent = joined
            .parent()
            .ok_or_else(|| anyhow::anyhow!("caminho sem pasta-pai: {path}"))?;
        while !parent.exists() {
            parent = parent
                .parent()
                .ok_or_else(|| anyhow::anyhow!("caminho sem ancestral existente: {path}"))?;
        }
        let parent = parent
            .canonicalize()
            .with_context(|| format!("não consegui resolver a pasta de {path}"))?;
        if !parent.starts_with(&self.root) {
            bail!("caminho fora da raiz do projeto: {path}");
        }
        if joined.exists() {
            let canonical = joined
                .canonicalize()
                .with_context(|| format!("não consegui resolver: {path}"))?;
            if !canonical.starts_with(&self.root) {
                bail!("caminho fora da raiz do projeto: {path}");
            }
        }
        Ok(joined)
    }

    pub fn read(&self, path: &str) -> Result<String> {
        let path = self.resolve_existing(path)?;
        fs::read_to_string(&path).with_context(|| format!("a ler {}", path.display()))
    }

    pub fn write(&self, path: &str, contents: &str) -> Result<()> {
        let target = self.resolve_for_write(path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("a criar {}", parent.display()))?;
        }
        viber::scene::doc::write_atomic(&target, contents.as_bytes())?;
        Ok(())
    }

    pub fn exists(&self, path: &str) -> bool {
        self.resolve_existing(path).is_ok()
    }

    pub fn mkdir(&self, path: &str) -> Result<()> {
        let joined = self.relative(path)?;
        fs::create_dir_all(&joined).with_context(|| format!("a criar {path}"))?;
        let canonical = joined.canonicalize()?;
        if !canonical.starts_with(&self.root) {
            bail!("caminho fora da raiz do projeto: {path}");
        }
        Ok(())
    }

    /// Lista uma pasta (não recursivo). Caminhos relativos à raiz, com `/`.
    pub fn list(&self, path: &str) -> Result<Vec<FsEntry>> {
        let dir = if path.trim().is_empty() {
            self.root.clone()
        } else {
            self.resolve_existing(path)?
        };
        if !dir.is_dir() {
            bail!("não é uma pasta: {path}");
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("a listar {path}"))? {
            let entry = entry?;
            // Symlinks podem apontar para fora: não entram na listagem.
            if entry.file_type()?.is_symlink() {
                continue;
            }
            let full = entry.path();
            let Ok(relative) = full.strip_prefix(&self.root) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.push(FsEntry {
                name,
                path: relative.to_string_lossy().replace('\\', "/"),
                dir: entry.file_type()?.is_dir(),
            });
            if entries.len() >= 5000 {
                break;
            }
        }
        entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.yaml"), "title: T\n").unwrap();
        fs::create_dir_all(dir.path().join("quests")).unwrap();
        fs::write(dir.path().join("quests/q.json"), "[]").unwrap();
        dir
    }

    #[test]
    fn reads_writes_and_lists_inside_the_root() {
        let dir = project();
        let fs = ProjectFs::new(dir.path());
        assert_eq!(fs.read("quests/q.json").unwrap(), "[]");
        fs.write("quests/q.json", "[1]").unwrap();
        assert_eq!(fs.read("quests/q.json").unwrap(), "[1]");
        fs.write("quests/new.json", "{}").unwrap();
        assert!(fs.exists("quests/new.json"));
        let list = fs.list("quests").unwrap();
        assert!(list.iter().any(|entry| entry.name == "new.json"));
        assert!(list.iter().all(|entry| !entry.dir));
    }

    #[test]
    fn rejects_parent_and_absolute_paths() {
        let dir = project();
        let fs = ProjectFs::new(dir.path());
        assert!(fs.read("../config.yaml").is_err());
        assert!(fs.read("/etc/passwd").is_err());
        assert!(fs.write("../escape.json", "{}").is_err());
        assert!(fs.write("/tmp/escape.json", "{}").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape_the_root() {
        let dir = project();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "segredo").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("quests/link.txt"),
        )
        .unwrap();
        let fs = ProjectFs::new(dir.path());
        assert!(fs.read("quests/link.txt").is_err());
        let list = fs.list("quests").unwrap();
        assert!(!list.iter().any(|entry| entry.name == "link.txt"));
    }

    #[test]
    fn write_is_atomic_and_keeps_a_backup() {
        let dir = project();
        let fs = ProjectFs::new(dir.path());
        fs.write("quests/q.json", "[2]").unwrap();
        assert!(dir.path().join("quests/q.bak").is_file());
        assert_eq!(fs.read("quests/q.json").unwrap(), "[2]");
    }

    #[test]
    fn write_creates_missing_parents_inside_the_root() {
        let dir = project();
        let fs = ProjectFs::new(dir.path());
        fs.write("data/novo/ficheiro.json", "{}").unwrap();
        assert!(dir.path().join("data/novo/ficheiro.json").is_file());
    }
}
