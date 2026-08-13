mod file_picker;
mod package;
mod theme;

use anyhow::{Context, Result};
use file_picker::PickerEntry;
use package::{create_translation_package, preserves_tokens, Package, ResourceKey, StringEntry, StblTable};
use rayon::prelude::*;
use std::path::PathBuf;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
slint::include_modules!();

const PACKAGE_EXTENSION: &str = "package";

#[derive(Clone, Copy, PartialEq)]
enum PickerMode {
    Open,
    Save,
}

/// Estado do file picker próprio do app (substitui o `rfd::FileDialog` nativo — ver
/// `src/file_picker.rs`). `entries` guarda a última listagem pra resolver o nome que a UI manda
/// de volta em `picker-row-clicked` sem precisar reler o diretório do disco.
struct PickerState {
    mode: PickerMode,
    current_dir: PathBuf,
    entries: Vec<PickerEntry>,
}

impl Default for PickerState {
    fn default() -> Self {
        Self { mode: PickerMode::Open, current_dir: file_picker::home_dir(), entries: Vec::new() }
    }
}

#[derive(Clone)]
struct TranslationEntry {
    key: u32,
    flags: u8,
    source: String,
    translation: String,
    table_key: ResourceKey,
}

#[derive(Default)]
struct AppState {
    source_path: Option<PathBuf>,
    entries: Vec<TranslationEntry>,
    search: String,
    memory: TranslationMemory,
    picker: PickerState,
    /// Pacote já validado esperando o usuário escolher onde salvar no picker (modo Save).
    pending_export: Option<Vec<StblTable>>,
}

/// Memória de tradução persistente: lembra traduções já aprovadas (por motor/tom)
/// entre mods diferentes e entre sessões do app, para nunca traduzir o mesmo texto duas vezes.
#[derive(Default)]
struct TranslationMemory {
    // "motor::tom" -> (texto original -> tradução)
    buckets: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

impl TranslationMemory {
    fn file_path() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".local/share/sims4-mod-translator/translation_memory.json"))
    }

    fn load() -> Self {
        let buckets = Self::file_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { buckets }
    }

    fn save(&self) {
        let Some(path) = Self::file_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.buckets) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Cópia do bucket de um motor/tom específico, pra consulta sem segurar o lock do estado.
    fn snapshot(&self, bucket: &str) -> std::collections::HashMap<String, String> {
        self.buckets.get(bucket).cloned().unwrap_or_default()
    }

    fn learn(&mut self, bucket: &str, entries: std::collections::HashMap<String, String>) {
        if entries.is_empty() {
            return;
        }
        self.buckets.entry(bucket.to_string()).or_default().extend(entries);
        self.save();
    }
}

fn main() -> Result<()> {
    let ui = MainWindow::new()?;
    let mut initial_state = AppState::default();
    initial_state.memory = TranslationMemory::load();
    let state = Arc::new(Mutex::new(initial_state));
    ui.set_status_text("Abra um mod .package para iniciar a revisão.".into());
    ui.set_file_name("Nenhum pacote carregado".into());
    theme::spawn_watcher(&ui);
    bind_callbacks(&ui, state);
    ui.run()?;
    Ok(())
}

fn bind_callbacks(ui: &MainWindow, state: Arc<Mutex<AppState>>) {
    ui.on_open_package({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let start_dir = state
                .lock()
                .expect("App state poisoned")
                .source_path
                .as_ref()
                .and_then(|p| p.parent().map(PathBuf::from))
                .unwrap_or_else(file_picker::home_dir);
            let state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                open_picker(&ui, &state, PickerMode::Open, start_dir, String::new());
            });
        }
    });

    ui.on_picker_row_clicked({
        let ui = ui.as_weak();
        let state = state.clone();
        move |name| {
            let name = name.to_string();
            let is_dir = {
                let guard = state.lock().expect("App state poisoned");
                guard.picker.entries.iter().find(|e| e.name == name).map(|e| e.is_dir)
            };
            let Some(is_dir) = is_dir else { return };
            let state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                if is_dir {
                    let next_dir = {
                        let mut guard = state.lock().expect("App state poisoned");
                        guard.picker.current_dir = guard.picker.current_dir.join(&name);
                        guard.picker.current_dir.clone()
                    };
                    refresh_picker(&ui, &state, next_dir);
                } else {
                    ui.set_picker_selected(name.clone().into());
                    if ui.get_picker_mode_save() {
                        ui.set_picker_filename(name.into());
                    }
                }
            });
        }
    });

    ui.on_picker_go_up({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                let next_dir = {
                    let mut guard = state.lock().expect("App state poisoned");
                    let parent = guard.picker.current_dir.parent().map(PathBuf::from).unwrap_or_else(|| guard.picker.current_dir.clone());
                    guard.picker.current_dir = parent.clone();
                    parent
                };
                refresh_picker(&ui, &state, next_dir);
            });
        }
    });

    ui.on_picker_go_home({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| picker_navigate_shortcut(&ui, &state, file_picker::home_dir()));
        }
    });

    ui.on_picker_go_downloads({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| picker_navigate_shortcut(&ui, &state, file_picker::downloads_dir()));
        }
    });

    ui.on_picker_go_mods({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| picker_navigate_shortcut(&ui, &state, file_picker::sims4_mods_dir()));
        }
    });

    ui.on_picker_filename_changed({
        let ui = ui.as_weak();
        move |text| {
            let _ = ui.upgrade_in_event_loop(move |ui| ui.set_picker_filename(text));
        }
    });

    ui.on_picker_cancel({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            state.lock().expect("App state poisoned").pending_export = None;
            let _ = ui.upgrade_in_event_loop(move |ui| ui.set_picker_visible(false));
        }
    });

    ui.on_picker_confirm({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let Some(ui_handle) = ui.upgrade() else { return };
            let mode = state.lock().expect("App state poisoned").picker.mode;
            match mode {
                PickerMode::Open => {
                    let selected = ui_handle.get_picker_selected().to_string();
                    if selected.is_empty() {
                        return;
                    }
                    let path = state.lock().expect("App state poisoned").picker.current_dir.join(&selected);
                    match load_package(&path, &state) {
                        Ok(count) => {
                            let inner_state = state.clone();
                            ui_handle.set_file_name(path.file_name().unwrap_or_default().to_string_lossy().into_owned().into());
                            ui_handle.set_status_text(format!("{count} textos carregados. Revise ou traduza antes de exportar.").into());
                            ui_handle.set_total_strings(count as i32);
                            rebuild_rows(&ui_handle, &inner_state);
                            ui_handle.set_picker_visible(false);
                        }
                        Err(error) => ui_handle.set_picker_error(format!("Falha ao abrir: {error}").into()),
                    }
                }
                PickerMode::Save => {
                    let mut filename = ui_handle.get_picker_filename().to_string();
                    if filename.trim().is_empty() {
                        ui_handle.set_picker_error("Informe um nome de arquivo.".into());
                        return;
                    }
                    if !filename.to_lowercase().ends_with(".package") {
                        filename.push_str(".package");
                    }
                    let output = state.lock().expect("App state poisoned").picker.current_dir.join(&filename);
                    let tables = state.lock().expect("App state poisoned").pending_export.take();
                    let Some(tables) = tables else {
                        ui_handle.set_picker_error("Nada para exportar.".into());
                        return;
                    };
                    match write_export(&tables, &output) {
                        Ok(()) => {
                            ui_handle.set_status_text(format!("Pacote validado e salvo em {}", output.display()).into());
                            ui_handle.set_picker_visible(false);
                            ui_handle.set_picker_error("".into());
                        }
                        Err(error) => {
                            state.lock().expect("App state poisoned").pending_export = Some(tables);
                            ui_handle.set_picker_error(format!("Exportação bloqueada: {error}").into());
                        }
                    }
                }
            }
        }
    });

    ui.on_search_changed({
        let ui = ui.as_weak();
        let state = state.clone();
        move |query| {
            state.lock().expect("App state poisoned").search = query.to_string();
            let inner_state = state.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| rebuild_rows(&ui, &inner_state));
        }
    });

    ui.on_edit_translation({
        let state = state.clone();
        move |key, value| {
            if let Some(entry) = state
                .lock()
                .expect("App state poisoned")
                .entries
                .iter_mut()
                .find(|entry| entry.key == key as u32)
            {
                entry.translation = value.to_string();
            }
        }
    });

    ui.on_translate_all({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let entries = state.lock().expect("App state poisoned").entries.clone();
            if entries.is_empty() {
                set_status(&ui, "Abra um pacote antes de traduzir.".into());
                return;
            }
            let selected_engine = ui.upgrade().map(|u| u.get_selected_engine().to_string()).unwrap_or_else(|| "Google Translate".into());
            let selected_preset = ui.upgrade().map(|u| u.get_selected_preset().to_string()).unwrap_or_else(|| "Neutro PT-BR".into());
            let engine_name = selected_engine.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_busy(true);
                ui.set_status_text(format!("Traduzindo com {engine_name}. Os marcadores do jogo serão preservados.").into());
            });
            let thread_ui = ui.clone();
            let thread_state = state.clone();
            std::thread::spawn(move || {
                let total_items = entries.len();
                let completed_count = Arc::new(AtomicUsize::new(0));
                let start_time = Instant::now();
                let mut failures = 0;
                let mut skipped = 0;

                // Textos muito longos (créditos, blocos HTML) raramente precisam de tradução e
                // custam desproporcionalmente caro em tempo de chamada; mantemos no idioma original.
                const MAX_TRANSLATABLE_CHARS: usize = 500;

                let mut from_memory = 0;
                let translated: Vec<TranslationEntry> = if selected_engine == "LM Studio (Local API)" {
                    ensure_lmstudio_ready();
                    let prompt = get_system_prompt(&selected_preset);
                    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(120)).build().unwrap();
                    let memory_bucket = format!("lmstudio::{selected_preset}");

                    let (result, f, s, m) = translate_with_memory(
                        entries,
                        &thread_state,
                        &thread_ui,
                        &completed_count,
                        start_time,
                        &memory_bucket,
                        MAX_TRANSLATABLE_CHARS,
                        8,
                        |src| {
                            let body = serde_json::json!({
                                "messages": [
                                    { "role": "system", "content": &prompt },
                                    { "role": "user", "content": src }
                                ],
                                "temperature": 0.15,
                                "stream": false
                            });
                            let resp = client
                                .post("http://localhost:1234/v1/chat/completions")
                                .json(&body)
                                .send()
                                .ok()?
                                .error_for_status()
                                .ok()?;
                            let json: serde_json::Value = resp.json().ok()?;
                            let content = json["choices"][0]["message"]["content"].as_str()?.trim().to_string();
                            if preserves_tokens(src, &content) && content != "-" {
                                Some(content)
                            } else {
                                None
                            }
                        },
                    );
                    failures = f;
                    skipped = s;
                    from_memory = m;
                    result
                } else if selected_engine == "Google Translate" {
                    // Endpoint não-oficial: concorrência moderada (10) é rápida sem arriscar
                    // bloqueio por IP em arquivos grandes; testamos até 32 sem erro, mas ficamos
                    // conservadores para uso sustentado.
                    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(15)).build().unwrap();

                    let (result, f, s, m) = translate_with_memory(
                        entries,
                        &thread_state,
                        &thread_ui,
                        &completed_count,
                        start_time,
                        "google",
                        MAX_TRANSLATABLE_CHARS,
                        10,
                        |src| {
                            let value = translate_google(&client, src).ok()?;
                            if preserves_tokens(src, &value) {
                                Some(value)
                            } else {
                                None
                            }
                        },
                    );
                    failures = f;
                    skipped = s;
                    from_memory = m;
                    result
                } else if selected_engine == "Argos Translate (Offline NMT)" {
                    let progress_ui = thread_ui.clone();
                    let progress_completed = completed_count.clone();

                    let mut seen = std::collections::HashSet::new();
                    let mut unique_sources: Vec<String> = Vec::new();
                    for entry in &entries {
                        if entry.source.chars().count() > MAX_TRANSLATABLE_CHARS {
                            skipped += 1;
                            continue;
                        }
                        if seen.insert(entry.source.clone()) {
                            unique_sources.push(entry.source.clone());
                        }
                    }
                    let unique_total = unique_sources.len();

                    let memory_snapshot = thread_state.lock().expect("App state poisoned").memory.snapshot("argos");
                    let mut translation_map: std::collections::HashMap<String, String> =
                        std::collections::HashMap::with_capacity(unique_total);
                    let mut to_translate: Vec<String> = Vec::with_capacity(unique_total);
                    for source in unique_sources {
                        if let Some(cached) = memory_snapshot.get(&source) {
                            translation_map.insert(source, cached.clone());
                            from_memory += 1;
                        } else {
                            to_translate.push(source);
                        }
                    }
                    progress_completed.fetch_add(from_memory, Ordering::SeqCst);
                    update_progress(&progress_ui, from_memory, unique_total.max(1), start_time);

                    match translate_argos_batch(&to_translate) {
                        Ok(argos_results) => {
                            let mut learned = std::collections::HashMap::with_capacity(to_translate.len());
                            for (source, pt_text) in to_translate.iter().zip(argos_results) {
                                if preserves_tokens(source, &pt_text) {
                                    learned.insert(source.clone(), pt_text.clone());
                                    translation_map.insert(source.clone(), pt_text);
                                } else {
                                    failures += 1;
                                    translation_map.insert(source.clone(), source.clone());
                                }
                            }
                            let done = progress_completed.fetch_add(to_translate.len(), Ordering::SeqCst) + to_translate.len();
                            update_progress(&progress_ui, done, unique_total.max(1), start_time);
                            thread_state.lock().expect("App state poisoned").memory.learn("argos", learned);
                        }
                        Err(_) => {
                            failures += to_translate.len();
                            for source in &to_translate {
                                translation_map.insert(source.clone(), source.clone());
                            }
                        }
                    }

                    entries
                        .into_iter()
                        .map(|mut entry| {
                            if let Some(translation) = translation_map.get(&entry.source) {
                                entry.translation = translation.clone();
                            }
                            entry
                        })
                        .collect()
                } else {
                    // Ollama: único motor que ainda não passou por esta rodada de otimizações.
                    let progress_ui = thread_ui.clone();
                    let progress_completed = completed_count.clone();
                    entries
                        .into_iter()
                        .map(|mut entry| {
                            let res = translate_ollama(&entry.source, "llama3");
                            let done = progress_completed.fetch_add(1, Ordering::SeqCst) + 1;
                            update_progress(&progress_ui, done, total_items, start_time);
                            match res {
                                Ok(value) if preserves_tokens(&entry.source, &value) => {
                                    entry.translation = value;
                                    entry
                                }
                                _ => {
                                    failures += 1;
                                    entry.translation = entry.source.clone();
                                    entry
                                }
                            }
                        })
                        .collect()
                };
                thread_state.lock().expect("App state poisoned").entries = translated;
                let _ = thread_ui.upgrade_in_event_loop(move |ui| {
                    rebuild_rows(&ui, &thread_state);
                    ui.set_busy(false);
                    let mut status = match (failures, skipped) {
                        (0, 0) => "Tradução concluída. Revise as sugestões antes de exportar.".to_string(),
                        (0, s) => format!("Tradução concluída. {s} texto(s) muito longos (créditos/HTML) foram mantidos no idioma original."),
                        (f, 0) => format!("Tradução concluída. {f} texto(s) tiveram tokens alterados e foram mantidos em inglês."),
                        (f, s) => format!("Tradução concluída. {f} texto(s) tiveram tokens alterados e {s} texto(s) longos foram mantidos em inglês."),
                    };
                    if from_memory > 0 {
                        status.push_str(&format!(" {from_memory} vieram da memória de traduções anteriores."));
                    }
                    ui.set_status_text(status.into());
                });
            });
        }
    });

    ui.on_export_package({
        let ui = ui.as_weak();
        let state = state.clone();
        move || {
            let Some(ui_handle) = ui.upgrade() else { return };
            match validate_and_build_export(&state) {
                Ok((stbl_tables, start_dir, default_name)) => {
                    state.lock().expect("App state poisoned").pending_export = Some(stbl_tables);
                    open_picker(&ui_handle, &state, PickerMode::Save, start_dir, default_name);
                }
                Err(error) => ui_handle.set_status_text(format!("Exportação bloqueada: {error}").into()),
            }
        }
    });
}

/// Abre o file picker, listando `start_dir` de cara.
fn open_picker(ui: &MainWindow, state: &Arc<Mutex<AppState>>, mode: PickerMode, start_dir: PathBuf, filename: String) {
    {
        let mut guard = state.lock().expect("App state poisoned");
        guard.picker.mode = mode;
        guard.picker.current_dir = start_dir.clone();
    }
    ui.set_picker_mode_save(mode == PickerMode::Save);
    ui.set_picker_selected("".into());
    ui.set_picker_filename(filename.into());
    ui.set_picker_error("".into());
    ui.set_picker_visible(true);
    refresh_picker(ui, state, start_dir);
}

/// Navega para um atalho (Início/Downloads/Mods); se a pasta não existir, mostra erro no picker
/// em vez de listar um diretório inexistente.
fn picker_navigate_shortcut(ui: &MainWindow, state: &Arc<Mutex<AppState>>, dir: PathBuf) {
    if !dir.is_dir() {
        ui.set_picker_error(format!("Pasta não encontrada: {}", dir.display()).into());
        return;
    }
    state.lock().expect("App state poisoned").picker.current_dir = dir.clone();
    refresh_picker(ui, state, dir);
}

/// Relista `dir` e atualiza as propriedades do picker na UI (rows, caminho, seleção limpa).
fn refresh_picker(ui: &MainWindow, state: &Arc<Mutex<AppState>>, dir: PathBuf) {
    let entries = file_picker::list_dir(&dir, PACKAGE_EXTENSION);
    let rows: Vec<PickerRow> = entries
        .iter()
        .map(|e| PickerRow { name: e.name.clone().into(), is_dir: e.is_dir })
        .collect();
    state.lock().expect("App state poisoned").picker.entries = entries;
    ui.set_picker_current_path(dir.display().to_string().into());
    ui.set_picker_selected("".into());
    ui.set_picker_error("".into());
    ui.set_picker_rows(ModelRc::from(std::rc::Rc::new(VecModel::from(rows))));
}

fn load_package(path: &PathBuf, state: &Arc<Mutex<AppState>>) -> Result<usize> {
    let package = Package::open(path)?;
    let tables = package.all_string_tables()?;
    let mut total_count = 0;
    let mut entries = Vec::new();
    for (table_key, table_entries) in tables {
        total_count += table_entries.len();
        for entry in table_entries {
            entries.push(TranslationEntry {
                key: entry.key,
                flags: entry.flags,
                source: entry.value.clone(),
                translation: entry.value,
                table_key,
            });
        }
    }
    let mut state = state.lock().expect("App state poisoned");
    state.source_path = Some(path.clone());
    state.search.clear();
    state.entries = entries;
    Ok(total_count)
}

/// Valida as traduções atuais e monta as tabelas STBL prontas pra exportar, sem tocar em disco
/// nem perguntar onde salvar — isso é responsabilidade de `open_picker`/`on_picker_confirm`.
/// Retorna também a pasta do pacote original (ponto de partida do picker) e o nome sugerido.
fn validate_and_build_export(state: &Arc<Mutex<AppState>>) -> Result<(Vec<StblTable>, PathBuf, String)> {
    let state = state.lock().expect("App state poisoned");
    let source_path = state.source_path.clone().context("Nenhum pacote foi carregado.")?;
    if state.entries.is_empty() {
        anyhow::bail!("Nenhum texto para exportar.");
    }

    use std::collections::BTreeMap;
    let mut tables_map: BTreeMap<ResourceKey, Vec<StringEntry>> = BTreeMap::new();

    for entry in &state.entries {
        if !preserves_tokens(&entry.source, &entry.translation) {
            anyhow::bail!("A chave 0x{:08X} removeu ou alterou um marcador do jogo.", entry.key);
        }
        tables_map.entry(entry.table_key).or_default().push(StringEntry {
            key: entry.key,
            flags: entry.flags,
            value: entry.translation.clone(),
        });
    }
    drop(state);

    let stbl_tables: Vec<StblTable> = tables_map
        .into_iter()
        .map(|(key, entries)| StblTable { key, entries })
        .collect();

    let start_dir = source_path.parent().map(PathBuf::from).unwrap_or_else(file_picker::home_dir);
    let default_name = source_path
        .file_stem()
        .map(|stem| format!("{}_PTBR_Translation.package", stem.to_string_lossy()))
        .unwrap_or_else(|| "Mod_PTBR_Translation.package".to_owned());
    Ok((stbl_tables, start_dir, default_name))
}

/// Escreve as tabelas já validadas no caminho escolhido pelo usuário no picker, com a mesma
/// checagem de round-trip (reabrir e reler) que o app sempre fez antes de considerar exportado.
fn write_export(stbl_tables: &[StblTable], output: &PathBuf) -> Result<()> {
    std::fs::write(output, create_translation_package(stbl_tables)?)
        .with_context(|| format!("Não foi possível salvar {}", output.display()))?;
    Package::open(output)?.all_string_tables()?;
    Ok(())
}

fn rebuild_rows(ui: &MainWindow, state: &Arc<Mutex<AppState>>) {
    let state = state.lock().expect("App state poisoned");
    let query = state.search.to_lowercase();
    let rows = state
        .entries
        .iter()
        .filter(|entry| {
            query.is_empty()
                || format!("0x{:08X}", entry.key).to_lowercase().contains(&query)
                || entry.source.to_lowercase().contains(&query)
                || entry.translation.to_lowercase().contains(&query)
        })
        .map(|entry| TranslationRow {
            key_hex: format!("0x{:08X}", entry.key).into(),
            key: entry.key as i32,
            source: SharedString::from(&entry.source),
            translation: SharedString::from(&entry.translation),
            token_state: if preserves_tokens(&entry.source, &entry.translation) { "Seguro".into() } else { "Revise tokens".into() },
            is_safe: preserves_tokens(&entry.source, &entry.translation),
        })
        .collect::<Vec<_>>();
    ui.set_rows(ModelRc::from(std::rc::Rc::new(VecModel::from(rows))));
}

fn set_status(ui: &slint::Weak<MainWindow>, message: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_status_text(message.into()));
}

/// Traduz uma lista de entradas com deduplicação, consulta/aprendizado da memória de
/// tradução, execução paralela e uma segunda tentativa automática por item que falha.
/// Usado pelos motores que traduzem item a item (LM Studio e Google Translate) — o Argos
/// tem um fluxo próprio porque traduz o lote inteiro num único processo Python.
/// Retorna (entradas traduzidas, falhas, textos pulados por serem longos demais, hits de memória).
fn translate_with_memory(
    entries: Vec<TranslationEntry>,
    thread_state: &Arc<Mutex<AppState>>,
    progress_ui: &slint::Weak<MainWindow>,
    completed_count: &Arc<AtomicUsize>,
    start_time: Instant,
    memory_bucket: &str,
    max_translatable_chars: usize,
    num_threads: usize,
    translate_fn: impl Fn(&str) -> Option<String> + Sync,
) -> (Vec<TranslationEntry>, usize, usize, usize) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()
        .unwrap_or_else(|_| rayon::ThreadPoolBuilder::new().build().unwrap());

    let mut skipped = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut unique_sources: Vec<String> = Vec::new();
    for entry in &entries {
        if entry.source.chars().count() > max_translatable_chars {
            skipped += 1;
            continue;
        }
        if seen.insert(entry.source.clone()) {
            unique_sources.push(entry.source.clone());
        }
    }
    let unique_total = unique_sources.len();

    let memory_snapshot = thread_state.lock().expect("App state poisoned").memory.snapshot(memory_bucket);

    let translation_map: Mutex<std::collections::HashMap<String, String>> =
        Mutex::new(std::collections::HashMap::with_capacity(unique_total));
    let mut to_translate: Vec<String> = Vec::with_capacity(unique_total);
    let mut from_memory = 0usize;
    {
        let mut map = translation_map.lock().expect("Translation map poisoned");
        for source in unique_sources {
            if let Some(cached) = memory_snapshot.get(&source) {
                map.insert(source, cached.clone());
                from_memory += 1;
            } else {
                to_translate.push(source);
            }
        }
    }
    completed_count.fetch_add(from_memory, Ordering::SeqCst);
    update_progress(progress_ui, from_memory, unique_total.max(1), start_time);

    let fail_atomic = std::sync::atomic::AtomicUsize::new(0);
    let learned: Mutex<std::collections::HashMap<String, String>> = Mutex::new(std::collections::HashMap::new());

    pool.install(|| {
        to_translate.par_iter().for_each(|source| {
            // Falhas costumam ser estocásticas; uma segunda tentativa recupera a maioria
            // delas antes de desistir e manter o texto original.
            let final_text = match translate_fn(source).or_else(|| translate_fn(source)) {
                Some(text) => {
                    learned.lock().expect("Learned map poisoned").insert(source.clone(), text.clone());
                    text
                }
                None => {
                    fail_atomic.fetch_add(1, Ordering::Relaxed);
                    source.clone()
                }
            };
            translation_map.lock().expect("Translation map poisoned").insert(source.clone(), final_text);
            let done = completed_count.fetch_add(1, Ordering::SeqCst) + 1;
            update_progress(progress_ui, done, unique_total.max(1), start_time);
        });
    });

    let failures = fail_atomic.into_inner();
    thread_state
        .lock()
        .expect("App state poisoned")
        .memory
        .learn(memory_bucket, learned.into_inner().expect("Learned map poisoned"));

    let map = translation_map.into_inner().expect("Translation map poisoned");
    let translated = entries
        .into_iter()
        .map(|mut entry| {
            if let Some(translation) = map.get(&entry.source) {
                entry.translation = translation.clone();
            }
            entry
        })
        .collect();

    (translated, failures, skipped, from_memory)
}

fn translate_google(client: &reqwest::blocking::Client, text: &str) -> Result<String> {
    // Recebe o client já construído: montar um Client novo por chamada custa uma negociação
    // TLS/TCP inteira (~80ms medidos) que a conexão reaproveitada evita.
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=en&tl=pt&dt=t&q={}",
        urlencoding::encode(text)
    );
    let response: serde_json::Value = client.get(url).send()?.error_for_status()?.json()?;
    let parts = response[0].as_array().context("Resposta inesperada do Google Translate.")?;
    Ok(parts.iter().filter_map(|part| part[0].as_str()).collect())
}

fn translate_ollama(text: &str, model: &str) -> Result<String> {
    let prompt = format!(
        "Translate from English to Portuguese (pt-BR). Keep all markers like {{0.String}}, <font>, <b>, \\n intact. Output ONLY translation.\nText: {}",
        text
    );
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false
    });
    let response: serde_json::Value = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?
        .post("http://localhost:11434/api/generate")
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;
    let translated = response["response"]
        .as_str()
        .context("Resposta inesperada do Ollama.")?
        .trim()
        .to_owned();
    Ok(translated)
}

const LMSTUDIO_MODEL: &str = "hy-mt2-1.8b";
const LMSTUDIO_CONTEXT_LENGTH: &str = "8192";
const LMSTUDIO_PARALLEL: &str = "8";

fn find_lms_binary() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let candidate = PathBuf::from(home).join(".lmstudio/bin/lms");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    if std::process::Command::new("lms").arg("--help").output().map(|o| o.status.success()).unwrap_or(false) {
        return Some(PathBuf::from("lms"));
    }
    None
}

/// Garante que o servidor LM Studio está com o modelo, contexto e paralelismo validados
/// (hy-mt2-1.8b, ctx 8192, parallel 8, GPU CUDA) antes de traduzir. Sem isso, o modelo pode
/// ficar carregado com uma config diferente (ex: deixada por outro app) e a tradução roda
/// mais lenta ou menos confiável do que o esperado. Falha silenciosamente se `lms` não
/// estiver instalado ou o servidor estiver fora do ar — a tradução segue com o que já estiver
/// carregado em vez de travar o usuário.
fn ensure_lmstudio_ready() {
    let Some(lms_bin) = find_lms_binary() else { return };

    let models: Vec<serde_json::Value> = std::process::Command::new(&lms_bin)
        .args(["ps", "--json"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| serde_json::from_slice::<serde_json::Value>(&out.stdout).ok())
        .and_then(|json| json.as_array().cloned())
        .unwrap_or_default();

    // "identifier" ganha sufixo (ex: "hy-mt2-1.8b:2") quando já existe outra instância do
    // mesmo modelo carregada; "modelKey" é o nome estável do modelo em si.
    let matches_target = |m: &serde_json::Value| m["modelKey"].as_str() == Some(LMSTUDIO_MODEL);
    let is_correctly_configured = |m: &serde_json::Value| {
        matches_target(m) && m["contextLength"].as_u64() == Some(8192) && m["parallel"].as_u64() == Some(8)
    };

    // Descarrega instâncias do mesmo modelo com config errada, pra não acumular VRAM com duplicatas.
    for identifier in models.iter().filter(|m| matches_target(m) && !is_correctly_configured(m)).filter_map(|m| m["identifier"].as_str()) {
        let _ = std::process::Command::new(&lms_bin).args(["unload", identifier]).output();
    }

    if models.iter().any(is_correctly_configured) {
        return;
    }

    let _ = std::process::Command::new(&lms_bin)
        .args([
            "load",
            LMSTUDIO_MODEL,
            "--context-length",
            LMSTUDIO_CONTEXT_LENGTH,
            "--parallel",
            LMSTUDIO_PARALLEL,
            "--gpu",
            "max",
            "-y",
        ])
        .output();
}

fn get_argos_python() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let venv_py = PathBuf::from(home).join("sims4-translator-env/bin/python3");
        if venv_py.exists() {
            return Some(venv_py);
        }
    }
    let py = PathBuf::from("/usr/bin/python3");
    if py.exists() {
        return Some(py);
    }
    None
}

fn translate_argos_batch(sources: &[String]) -> Result<Vec<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let python_bin = get_argos_python().context("Python do Argos Translate não foi encontrado.")?;
    let mut child = Command::new(python_bin)
        .arg("-c")
        .arg(
            r#"
import sys, json, re
from concurrent.futures import ThreadPoolExecutor
import argostranslate.translate

# Protege {0.String}/<tags> antes de traduzir: um NMT tradicional não entende instruções
# como "preserve isso", então precisa que o marcador vire algo que ele nunca vai querer
# alterar. Testamos alguns formatos de máscara: um token puramente numérico sobrevive à
# tradução muito melhor (98%) do que um token com letras tipo __SIMS_TOK_0__ (92%), porque
# motores de tradução são treinados a preservar números literais mas não pseudo-palavras.
PLACEHOLDER_REGEX = re.compile(r'\{[^}]+\}')
HTML_TAG_REGEX = re.compile(r'</?[a-zA-Z0-9]+[^>]*>')

def mask(text):
    tokens = []
    def replace(m):
        idx = len(tokens)
        tokens.append(m.group(0))
        return f"9{idx:05d}9"
    masked = HTML_TAG_REGEX.sub(replace, text)
    masked = PLACEHOLDER_REGEX.sub(replace, masked)
    return masked, tokens

def unmask(masked_text, tokens):
    result = masked_text
    for idx, token in enumerate(tokens):
        pattern = re.compile(r'\s*9\s*' + f'{idx:05d}' + r'\s*9\s*')
        result = pattern.sub(token, result)
    return result

def translate_one(text):
    masked, tokens = mask(text)
    translated = argostranslate.translate.translate(masked, 'en', 'pt')
    return unmask(translated, tokens)

try:
    data = json.load(sys.stdin)
    # Threading dá ~25-30% de ganho medido: o motor libera o GIL durante a tradução.
    with ThreadPoolExecutor(max_workers=4) as ex:
        translated = list(ex.map(translate_one, data))
    print(json.dumps(translated))
except Exception as e:
    sys.exit(1)
"#,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Falha ao iniciar o processo do Argos Translate.")?;

    let json_input = serde_json::to_string(sources)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(json_input.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        anyhow::bail!("Erro durante a execução do Argos Translate.");
    }

    let results: Vec<String> = serde_json::from_slice(&output.stdout)?;
    if results.len() != sources.len() {
        anyhow::bail!("Quantidade de traduções incorreta.");
    }

    Ok(results)
}

fn get_system_prompt(preset: &str) -> String {
    let base = match preset {
        "Adulto +18 (WickedWhims)" => "Traduza para PT-BR adulto/erótico. Use gírias reais, sem censura.",
        "Humor & Sarcasmo" => "Traduza para PT-BR com humor, sarcasmo e expressões coloquiais engraçadas.",
        "Fantasia & Épico" => "Traduza para PT-BR com tom elegante, medieval e místico.",
        _ => "Traduza para PT-BR natural de jogos.",
    };

    // Textos curtos e sem contexto (rótulos de menu, categorias, traços) fazem modelos
    // pequenos "completarem" ou inventarem conteúdo em vez de traduzir literalmente.
    // As regras abaixo (testadas empiricamente) reduzem drasticamente essa alucinação.
    format!(
        "{base} Traduza apenas o texto dado — nunca invente informação nem complete frases, \
         mesmo que o texto pareça curto ou sem contexto. Nunca adicione tags HTML ou variáveis \
         que não estejam no original; preserve exatamente as que existirem. Retorne SOMENTE a \
         tradução, sem aspas ou comentários."
    )
}

fn update_progress(
    ui: &slint::Weak<MainWindow>,
    completed: usize,
    total: usize,
    start_time: Instant,
) {
    let pct = (completed as f32 / total as f32).clamp(0.0, 1.0);
    let elapsed = start_time.elapsed().as_secs_f32();
    let eta_text = if completed > 0 && completed < total {
        let rate = completed as f32 / elapsed;
        let remaining = total - completed;
        let eta_secs = (remaining as f32 / rate) as u32;
        if eta_secs > 60 {
            format!("~{}m {}s restantes", eta_secs / 60, eta_secs % 60)
        } else {
            format!("~{}s restantes", eta_secs)
        }
    } else {
        "finalizando...".to_owned()
    };

    let text = format!("{:.0}% • {} / {} frases • {}", pct * 100.0, completed, total, eta_text);
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_progress_percentage(pct);
        ui.set_progress_text(text.into());
    });
}

#[cfg(test)]
mod app_tests {
    use super::*;

    #[test]
    fn translation_memory_round_trip() {
        let mut memory = TranslationMemory::default();
        let mut entries = std::collections::HashMap::new();
        entries.insert("Hello".to_string(), "Olá".to_string());
        memory.buckets.insert("test::bucket".to_string(), entries);

        let snapshot = memory.snapshot("test::bucket");
        assert_eq!(snapshot.get("Hello"), Some(&"Olá".to_string()));
        assert!(memory.snapshot("missing::bucket").is_empty());
    }

    #[test]
    #[ignore = "requer um servidor LM Studio real rodando com hy-mt2-1.8b baixado"]
    fn ensure_lmstudio_ready_fixes_wrong_config() {
        ensure_lmstudio_ready();
        let lms_bin = find_lms_binary().expect("CLI 'lms' não encontrado");
        let output = std::process::Command::new(&lms_bin)
            .args(["ps", "--json"])
            .output()
            .expect("falha ao rodar 'lms ps'");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("saída inválida do 'lms ps'");
        let models = json.as_array().expect("formato inesperado");
        assert!(
            models.iter().any(|m| {
                m["modelKey"].as_str() == Some(LMSTUDIO_MODEL)
                    && m["contextLength"].as_u64() == Some(8192)
                    && m["parallel"].as_u64() == Some(8)
            }),
            "modelo não foi recarregado com a config validada: {json}"
        );
        assert_eq!(models.len(), 1, "instâncias duplicadas do modelo não foram limpas: {json}");
    }

    #[test]
    #[ignore = "requer o venv do Argos Translate (~/sims4-translator-env) instalado"]
    fn argos_batch_preserves_tags_and_dedup_shape() {
        let sources = vec![
            "Passionate Groping".to_string(),
            "{0.SimFirstName} dislikes the green hair color.".to_string(),
            "<b>@AdviserBot:</b> 'Hello, Sim!'".to_string(),
        ];
        let results = translate_argos_batch(&sources).expect("tradução via Argos falhou");
        assert_eq!(results.len(), sources.len());
        let ok = sources.iter().zip(&results).filter(|(s, t)| preserves_tokens(s, t)).count();
        assert!(ok >= 2, "esperava ao menos 2/3 preservando tokens, obteve {ok}/3: {results:?}");

        // Lote vazio (todo mundo veio da memória) não deve tentar abrir um processo Python.
        assert_eq!(translate_argos_batch(&[]).unwrap(), Vec::<String>::new());
    }

    #[test]
    #[ignore = "requer acesso à rede (translate.googleapis.com)"]
    fn google_translate_reuses_client_and_preserves_tags() {
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(15)).build().unwrap();
        let source = "{0.SimFirstName} dislikes the green hair color.";
        let translated = translate_google(&client, source).expect("tradução via Google falhou");
        assert!(preserves_tokens(source, &translated), "tokens não preservados: {translated:?}");
        // O mesmo client reaproveitado numa segunda chamada também deve funcionar.
        let translated2 = translate_google(&client, "Stop").expect("segunda chamada com o mesmo client falhou");
        assert!(!translated2.is_empty());
    }
}
