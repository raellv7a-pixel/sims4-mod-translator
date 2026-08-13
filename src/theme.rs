use serde_json::Value;
use slint::{Color, ComponentHandle};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::{MainWindow, Theme};

const POLL_INTERVAL: Duration = Duration::from_millis(700);

/// Observa os arquivos de tema exportados pela Hydra Shell (fork da Noctalia) e aplica as
/// cores ao vivo no `global Theme` do Slint, com transições suaves (as animações ficam do
/// lado do .slint). Dois níveis de fonte, do mais específico ao mais genérico:
///
/// 1. `~/.config/sims4-mod-translator/theme.json` — gerado pelo template nativo registrado em
///    `TemplateRegistry.qml` da Hydra Shell (papéis já nomeados igual aos tokens do `Theme`).
/// 2. `~/.config/noctalia/colors.json` — sempre presente quando a shell está instalada
///    (16 cores M3 reescritas a cada troca de esquema), usado como fallback derivado.
///
/// Sem nenhum dos dois arquivos (Hydra Shell não instalada), nada é sobrescrito e os valores
/// compilados em `ui/theme.slint` (paleta atual do app) continuam valendo — zero regressão
/// para quem roda o app sem a shell.
///
/// Feito por polling de mtime em vez de um watcher baseado em evento (ex: crate `notify`) para
/// não introduzir uma dependência nova: o app já usa `std::thread::spawn` para todo trabalho em
/// background (tradução), então este segue o mesmo padrão.
pub fn spawn_watcher(ui: &MainWindow) {
    let weak = ui.as_weak();
    apply_current(&weak);

    std::thread::spawn(move || {
        let mut last_custom = None;
        let mut last_fallback = None;
        loop {
            std::thread::sleep(POLL_INTERVAL);
            let custom_mtime = mtime(&custom_theme_path());
            let fallback_mtime = mtime(&noctalia_colors_path());
            if custom_mtime != last_custom || fallback_mtime != last_fallback {
                last_custom = custom_mtime;
                last_fallback = fallback_mtime;
                apply_current(&weak);
            }
        }
    });
}

fn apply_current(weak: &slint::Weak<MainWindow>) {
    let mut tokens: HashMap<&'static str, Color> = HashMap::new();
    if let Some(colors) = load_fallback(&noctalia_colors_path()) {
        tokens.extend(colors);
    }
    if let Some(colors) = load_custom(&custom_theme_path()) {
        tokens.extend(colors);
    }
    if tokens.is_empty() {
        return;
    }
    let _ = weak.upgrade_in_event_loop(move |ui| apply_tokens(&ui, &tokens));
}

fn apply_tokens(ui: &MainWindow, tokens: &HashMap<&'static str, Color>) {
    let theme = ui.global::<Theme>();
    for (key, color) in tokens {
        match *key {
            "background" => theme.set_color_background(*color),
            "surface" => theme.set_color_surface(*color),
            "surface_container_low" => theme.set_color_surface_container_low(*color),
            "surface_container" => theme.set_color_surface_container(*color),
            "surface_container_high" => theme.set_color_surface_container_high(*color),
            "on_surface" => theme.set_color_on_surface(*color),
            "on_surface_variant" => theme.set_color_on_surface_variant(*color),
            "outline" => theme.set_color_outline(*color),
            "shadow" => theme.set_color_shadow(*color),
            "primary" => theme.set_color_primary(*color),
            "on_primary" => theme.set_color_on_primary(*color),
            "secondary" => theme.set_color_secondary(*color),
            "on_secondary" => theme.set_color_on_secondary(*color),
            "tertiary" => theme.set_color_tertiary(*color),
            "error_container" => theme.set_color_error_container(*color),
            "on_error_container" => theme.set_color_on_error_container(*color),
            "success_container" => theme.set_color_success_container(*color),
            "on_success_container" => theme.set_color_on_success_container(*color),
            _ => {}
        }
    }
}

fn custom_theme_path() -> PathBuf {
    config_home().join("sims4-mod-translator/theme.json")
}

fn noctalia_colors_path() -> PathBuf {
    config_home().join("noctalia/colors.json")
}

fn config_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default().join(".config")
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Lê o template nativo registrado na Hydra Shell (`Assets/Templates/sims4-mod-translator.json`),
/// que já sai com os papéis nomeados exatamente como os tokens do `Theme` — sem remapeamento.
fn load_custom(path: &PathBuf) -> Option<HashMap<&'static str, Color>> {
    let json = read_json(path)?;
    let mut map = HashMap::new();
    let fields: &[(&str, &str)] = &[
        ("colorBackground", "background"),
        ("colorSurface", "surface"),
        ("colorSurfaceContainerLow", "surface_container_low"),
        ("colorSurfaceContainer", "surface_container"),
        ("colorSurfaceContainerHigh", "surface_container_high"),
        ("colorOnSurface", "on_surface"),
        ("colorOnSurfaceVariant", "on_surface_variant"),
        ("colorOutline", "outline"),
        ("colorShadow", "shadow"),
        ("colorPrimary", "primary"),
        ("colorOnPrimary", "on_primary"),
        ("colorSecondary", "secondary"),
        ("colorOnSecondary", "on_secondary"),
        ("colorTertiary", "tertiary"),
        ("colorErrorContainer", "error_container"),
        ("colorOnErrorContainer", "on_error_container"),
        ("colorSuccessContainer", "success_container"),
        ("colorOnSuccessContainer", "on_success_container"),
    ];
    for (json_key, role) in fields {
        if let Some(color) = get_color(&json, json_key) {
            map.insert(*role, color);
        }
    }
    Some(map)
}

/// Lê o `colors.json` da Noctalia (sempre presente, 16 chaves `mXxx`) e deriva os papéis que
/// ele não tem de forma nativa (camadas de superfície, background) usando `.darker()`/`.brighter()`.
fn load_fallback(path: &PathBuf) -> Option<HashMap<&'static str, Color>> {
    let json = read_json(path)?;
    let m_primary = get_color(&json, "mPrimary")?;
    let m_on_primary = get_color(&json, "mOnPrimary")?;
    let m_secondary = get_color(&json, "mSecondary")?;
    let m_on_secondary = get_color(&json, "mOnSecondary")?;
    let m_tertiary = get_color(&json, "mTertiary")?;
    let m_error = get_color(&json, "mError")?;
    let m_surface = get_color(&json, "mSurface")?;
    let m_on_surface = get_color(&json, "mOnSurface")?;
    let m_surface_variant = get_color(&json, "mSurfaceVariant")?;
    let m_on_surface_variant = get_color(&json, "mOnSurfaceVariant")?;
    let m_outline = get_color(&json, "mOutline")?;
    let m_shadow = get_color(&json, "mShadow")?;

    let mut map = HashMap::new();
    map.insert("primary", m_primary);
    map.insert("on_primary", m_on_primary);
    map.insert("secondary", m_secondary);
    map.insert("on_secondary", m_on_secondary);
    map.insert("tertiary", m_tertiary);
    map.insert("outline", m_outline);
    map.insert("shadow", m_shadow);
    map.insert("surface", m_surface);
    map.insert("on_surface", m_on_surface);
    map.insert("surface_container", m_surface_variant);
    map.insert("on_surface_variant", m_on_surface_variant);
    map.insert("surface_container_low", m_surface.darker(0.12));
    map.insert("surface_container_high", m_surface_variant.brighter(0.08));
    map.insert("background", m_surface.darker(0.35));
    map.insert("error_container", m_error.darker(0.55));
    map.insert("on_error_container", m_error);
    map.insert("success_container", m_tertiary.darker(0.55));
    map.insert("on_success_container", m_tertiary);
    Some(map)
}

fn read_json(path: &PathBuf) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn get_color(json: &Value, key: &str) -> Option<Color> {
    parse_hex(json.get(key)?.as_str()?)
}

fn parse_hex(raw: &str) -> Option<Color> {
    let hex = raw.trim().trim_start_matches('#');
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::from_rgb_u8(r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(Color::from_argb_u8(a, r, g, b))
        }
        _ => None,
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn parses_six_and_eight_digit_hex() {
        let c = parse_hex("#9c6bff").expect("cor de 6 dígitos deve parsear");
        assert_eq!((c.red(), c.green(), c.blue()), (0x9c, 0x6b, 0xff));

        let c = parse_hex("121015aa").expect("cor de 8 dígitos deve parsear");
        assert_eq!((c.red(), c.green(), c.blue(), c.alpha()), (0x12, 0x10, 0x15, 0xaa));

        assert!(parse_hex("not-a-color").is_none());
    }

    #[test]
    fn loads_and_derives_fallback_roles_from_noctalia_colors_json() {
        let dir = std::env::temp_dir().join(format!("s4mt-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("colors.json");
        std::fs::write(
            &path,
            r##"{
                "mPrimary": "#ecb5e7", "mOnPrimary": "#492149",
                "mSecondary": "#dabed4", "mOnSecondary": "#3d2a3b",
                "mTertiary": "#ffb3b5", "mOnTertiary": "#551f23",
                "mError": "#ffb4ab", "mOnError": "#690005",
                "mSurface": "#161215", "mOnSurface": "#e9e0e4",
                "mSurfaceVariant": "#231e21", "mOnSurfaceVariant": "#d1c3cc",
                "mOutline": "#4e444c", "mShadow": "#000000",
                "mHover": "#ffb3b5", "mOnHover": "#551f23"
            }"##,
        )
        .unwrap();

        let map = load_fallback(&path).expect("deve carregar o colors.json de exemplo");
        assert_eq!(map.len(), 18);
        assert!(map.contains_key("background"));
        assert!(map.contains_key("surface_container_low"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
