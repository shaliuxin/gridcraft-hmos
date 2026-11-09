//! 系统语言与系统字体：把鸿蒙的 locale / 字体接进 gridcraft 上游。
//!
//! 语言（与 wordcraft 根本不同，须注意）：gridcraft 的界面翻译是**内嵌源码**的（`crates/ui-egui/src/i18n/*.rs`
//! 每个语言一个 `mod`，编译期 `translate()`），**没有** `CRAFT_I18N_DIR`/外部 tsv，也**没有**
//! `WORDCRAFT_LANGUAGE` 这类专属环境变量。「跟随系统」走 `i18n::Language::system()` →
//! `sys_locale::get_locale()`，在 Linux（含 OHOS）上读 `LC_ALL`/`LC_MESSAGES`/`LANGUAGE`/`LANG`，
//! 而鸿蒙应用进程不带这些变量，于是「跟随系统」永远解析成英文。ArkTS 侧本来就能拿到真实 locale
//! （`i18n.System.getSystemLocale()`），这里把 BCP-47 标签写进 `LANG`（及 `LANGUAGE`/`LC_ALL`/
//! `LC_MESSAGES`），`sys_locale` 便解析出正确语言，上游自己的「跟随系统」完整生效。
//!
//! 字体：gridcraft 的 CJK fallback（`theme/font_fallback.rs` 的 `LINUX_CJK_DIRS`）只扫
//! **22 个固定绝对路径**（`/usr/share/fonts/...`、`/run/host/fonts/...`），鸿蒙沙箱既没有、也
//! 写不进这些目录。真机实测（2026-11-07，HPR-W72）：即使把 `HarmonyOS_Sans_SC.ttf` 复制进
//! `$HOME/.fonts` 并命名成上游候选名，中文工具栏标签仍是满屏 tofu —— 因为那份目录只被
//! `system_font_dirs()`（字体目录 catalog）扫，CJK fallback 根本不看它。
//!
//! 因此真正的修复在宿主侧：`cjk_font_bytes()` 直接读出系统字体字节，由 `lib.rs` 用 egui 0.37
//! 的 `Context::add_font(FontInsert)` 以 `FontPriority::Lowest` 追加到所有字体族。
//! 不 patch 上游、不写系统分区、不动主题；`stage_fonts()` 仍保留作为其它代码路径的兜底。

use std::path::{Path, PathBuf};

/// 上游 `noto_cjk("sc", 2)` 认得的中文候选：分 locale 的 OTF 与区域合并 TTC（face 2 = 简体中文）。
const CJK_OTF: &str = "NotoSansCJKsc-Regular.otf";
const CJK_TTC: &str = "NotoSansCJK-Regular.ttc";
/// 上游 `cjk_han_japanese`/`cjk_hangul` 认得的候选名（同一合并 TTC，face 0=JP / 1=KR）。
const CJK_JP_OTF: &str = "NotoSansCJKjp-Regular.otf";
const CJK_KR_OTF: &str = "NotoSansCJKkr-Regular.otf";

/// 鸿蒙系统字体目录。
const SYSTEM_FONTS: &str = "/system/fonts";

/// 按界面语言挑系统 CJK 面。鸿蒙没有日/韩专用面，非繁体一律用 SC（含全部常用汉字）。
fn cjk_face(language: &str) -> &'static str {
    match language {
        "zh-TW" => "HarmonyOS_Sans_TC.ttf",
        "ja" => "HarmonyOS_Sans_SC.ttf",
        _ => "HarmonyOS_Sans_SC.ttf",
    }
}

/// 系统 locale 的 BCP-47 形式（`zh_CN` → `zh-CN`）。`locale` 缺失时退回 `language`。
fn system_tag(locale: &str, language: &str) -> Option<String> {
    let tag = [locale, language].into_iter().map(str::trim).find(|tag| !tag.is_empty())?;
    Some(tag.chars().map(|c| if c == '_' { '-' } else { c }).collect())
}

/// 在 `gc_init` 里、任何 UI 与字体读取之前调用一次。
///
/// 顺序有意义：先写环境变量，再让 `i18n::Language::system()` 首次求值（读 sys_locale，
/// 第一次读到什么就固定什么）。
pub fn install(files_dir: &str, locale: &str, language: &str) {
    let tag = system_tag(locale, language);
    if let Some(tag) = &tag {
        // sys-locale 按 LC_ALL → LC_MESSAGES → LANGUAGE → LANG 取值：先清掉可能残留的高优先级
        // 变量，再统一写成本标签，与桌面 Linux 用法一致。
        for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE"] {
            std::env::remove_var(key);
        }
        std::env::set_var("LANG", tag);
        std::env::set_var("LANGUAGE", tag);
    }
    // 把鸿蒙系统字体挂到 $HOME/.fonts（gridcraft 的 system_font_dirs() 在 linux 分支扫
    // /usr/share/fonts、$HOME/.fonts、$HOME/.local/share/fonts；但 CJK 回退用的是固定
    // LINUX_CJK_DIRS，不含 $HOME/.fonts——故此步对中文界面是 best-effort，英文界面不受影响）。
    //
    // 关键：gridcraft 的 system_font_dirs() 用 env::var("HOME") 拼路径，而 stage 的目标目录必须
    // 与之一致。OHOS 应用进程里 HOME 可能已被设成别的路径或空串——此时「挂进 files_dir/.fonts」
    // 和「从 $HOME/.fonts 找」就不是同一个目录，字体扫描必然找不到。因此这里**无条件**把 HOME
    // 指向 files_dir，再挂到 $HOME/.fonts，保证挂与找同目录。
    let prior_home = std::env::var_os("HOME").map(|s| s.to_string_lossy().into_owned());
    std::env::set_var("HOME", files_dir);
    log::info!("locale: prior HOME={prior_home:?} -> now {files_dir}");
    let tag_str = tag.as_deref().unwrap_or_default();
    let home = Path::new(files_dir);
    // 尽量命中 gridcraft 的 CJK 候选名（NotoSansCJK{locale}-Regular.otf 与合并 TTC）。
    let mut ok = false;
    for base in [home.join(".fonts"), home.join(".local/share/fonts")] {
        if let Ok(dir) = stage_fonts(&base, tag_str) {
            let staged = dir.join(CJK_OTF);
            if let Ok(meta) = std::fs::metadata(&staged) {
                log::info!(
                    "locale: tag={tag_str:?} HOME={} cjk staged at {} ({} bytes)",
                    home.display(), staged.display(), meta.len()
                );
                ok = true;
                break;
            }
        }
    }
    if !ok {
        log::warn!("locale: tag={tag_str:?} system CJK font NOT staged — interface Chinese may show as boxes");
    }
}

/// 读取鸿蒙系统 CJK 字体的字节，交给宿主用 `egui::Context::add_font` 注入 fallback。
///
/// 这是中文界面不出 tofu 的**关键路径**：上游 `load_fallback` 只扫固定的
/// `LINUX_CJK_DIRS`，沙箱写不进去；`stage_fonts()` 挂到 `$HOME/.fonts` 的副本
/// 对 CJK fallback 无效（`system_font_dirs()` 只把它当 catalog）。这里绕过路径扫描，
/// 直接把字节交给宿主注入 egui 的字体定义。
///
/// 字体按界面语言挑 SC/TC；`HarmonyOS_Sans_SC.ttf` 约 20 MB，只在启动时读一次。
pub fn cjk_font_bytes(language: &str) -> Option<Vec<u8>> {
    let source = Path::new(SYSTEM_FONTS).join(cjk_face(language));
    match std::fs::read(&source) {
        Ok(bytes) if !bytes.is_empty() => {
            log::info!(
                "locale: system CJK face {} ({} bytes) ready for fallback injection",
                source.display(),
                bytes.len()
            );
            Some(bytes)
        }
        Ok(_) => {
            log::warn!("locale: system CJK face {} is empty", source.display());
            None
        }
        Err(e) => {
            log::warn!("locale: cannot read {}: {e} — Chinese UI will show tofu", source.display());
            None
        }
    }
}

/// 把要用的系统字体挂到 `base`（= `$HOME/.fonts`）下，返回挂好的目录。
///
/// 鸿蒙沙箱可能禁止建软链（实测 EACCES），于是退回复制——只在第一次（或系统换了字体文件名）时
/// 发生，之后按文件名命中直接复用。
fn stage_fonts(base: &Path, language: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(base).map_err(|e| format!("create .fonts: {e}"))?;
    let cjk = cjk_face(language);
    // 按 gridcraft 的多个 CJK 候选名都放一份，任一命中即可。
    stage_one(base, CJK_OTF, cjk)?;
    stage_one(base, CJK_TTC, cjk)?;
    stage_one(base, CJK_JP_OTF, cjk)?;
    stage_one(base, CJK_KR_OTF, cjk)?;
    Ok(base.to_path_buf())
}

/// 在 `base` 下按 `rel` 放一份系统字体 `face`。
fn stage_one(base: &Path, rel: &str, face: &str) -> Result<(), String> {
    let source = Path::new(SYSTEM_FONTS).join(face);
    if !source.is_file() {
        return Err(format!("no {face} in {SYSTEM_FONTS}"));
    }
    let link = base.join(rel);
    // 断链：系统升级把字体文件改了名。
    if let Ok(meta) = std::fs::symlink_metadata(&link) {
        if meta.file_type().is_symlink() && !link.exists() {
            let _ = std::fs::remove_file(&link);
        }
    }
    if std::fs::symlink_metadata(&link).is_ok() {
        return Ok(());
    }
    // 沙箱常禁软链：退回复制。
    if std::os::unix::fs::symlink(&source, &link).is_err() {
        log::info!("locale: staging {} as {}", source.display(), link.display());
        std::fs::copy(&source, &link).map_err(|e| format!("copy {}: {e}", link.display()))?;
    }
    Ok(())
}
