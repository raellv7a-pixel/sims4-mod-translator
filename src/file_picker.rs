use std::path::{Path, PathBuf};

pub struct PickerEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Lista um diretório para o file picker: pastas primeiro (ordem alfabética, sem diferenciar
/// caixa), depois arquivos cuja extensão bate com `extension_filter` (ex: "package"). Arquivos
/// ocultos (dotfiles) são ignorados; entradas que não podem ser lidas (permissão, symlink
/// quebrado) são puladas silenciosamente em vez de derrubar a listagem inteira.
pub fn list_dir(dir: &Path, extension_filter: &str) -> Vec<PickerEntry> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let extension_filter = extension_filter.to_lowercase();

    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                dirs.push(PickerEntry { name, is_dir: true });
            } else if name.to_lowercase().ends_with(&extension_filter) {
                files.push(PickerEntry { name, is_dir: false });
            }
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);
    dirs
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn downloads_dir() -> PathBuf {
    home_dir().join("Downloads")
}

pub fn sims4_mods_dir() -> PathBuf {
    home_dir().join("Documents/Electronic Arts/The Sims 4/Mods")
}

#[cfg(test)]
mod file_picker_tests {
    use super::*;

    fn make_tree() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s4mt-picker-test-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::write(dir.join("mod_b.package"), b"").unwrap();
        std::fs::write(dir.join("Mod_A.package"), b"").unwrap();
        std::fs::write(dir.join("readme.txt"), b"").unwrap();
        std::fs::write(dir.join(".hidden.package"), b"").unwrap();
        dir
    }

    fn rand_suffix() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as u64
    }

    #[test]
    fn lists_dirs_before_files_case_insensitive_and_filters_extension() {
        let dir = make_tree();
        let entries = list_dir(&dir, "package");

        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Zeta", "Mod_A.package", "mod_b.package"]);
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir && !entries[3].is_dir);
        assert!(!names.contains(&"readme.txt"));
        assert!(!names.contains(&".hidden.package"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn returns_empty_for_missing_dir() {
        let entries = list_dir(&PathBuf::from("/path/that/does/not/exist"), "package");
        assert!(entries.is_empty());
    }
}
