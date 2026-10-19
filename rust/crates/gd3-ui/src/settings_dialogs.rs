//! 设置页三个覆盖层的**开合**：标头预设 / 身份预设 / 库分类。
//!
//! 第 117–119 步把设置页的「写回」半边接成了 [`SettingsWriter`]，但那九个
//! `*AddRequested` / `*EditRequested` / `*EditClosed` 动作一直走 `defer` 记成待办 ——
//! 它们不写 cfg，只改窗口上的 `headersEditVisible` / `headersEditIndex` 之类属性，
//! 而 [`SettingsWriter`] 不持窗口。这一步给它们单独一层。
//!
//! | Slint 回调 | Python 源 | 这里 |
//! |---|---|---|
//! | `headersPresetAddRequested` | `setting_cards.py:719-727` | [`open_headers`] + `None` |
//! | `headersPresetEditRequested` | `setting_cards.py:729-736` | [`open_headers`] + `Some(i)` |
//! | `headersPresetEditClosed` | 覆盖层「取消」 | [`close_headers`] |
//! | `identityPresetAddRequested` | `setting_cards.py:565-574` | [`open_identity`] + `None` |
//! | `identityPresetEditRequested` | `setting_cards.py:576-586` | [`open_identity`] + `Some(i)` |
//! | `identityPresetEditClosed` | 覆盖层「取消」 | [`close_identity`] |
//! | `categoryAddRequested` | `category_settings.py:129-133` | [`open_category_new`] |
//! | `categoryEditRequested` | `category_settings.py:135-142` | [`open_category`] |
//! | `categoryEditClosed` | 覆盖层「取消」 | [`close_category`] |
//!
//! 分工：`install_settings_actions`（`lib.rs`）装 `*EditAccepted` —— 提交草稿要
//! `SettingsActions`；这里装入口与取消，只动窗口属性。两者装的是**不同**的回调，
//! 互不覆盖。
//!
//! ## 两条越界守卫
//!
//! - **身份预设**：Python 的 `_onEditClicked` 自己守着 `if index < 0 or
//!   index >= len(presets): return`（`setting_cards.py:578-579`），照抄。
//! - **标头预设**：Python 那边没有守卫，直接 `presets[index]`（`:732`）—— 越界会抛
//!   `IndexError`。这里静默返回：Slint 的 `headersEditIndex` 是普通整数属性，越界
//!   索引只可能来自手改过的模型。
//! - **库分类**：`categoryById` 查不到就 `return`（`category_settings.py:137-138`），
//!   照抄。
//!
//! 列表长度一律从 [`SettingsWriter::get`] 的逻辑值取 —— 也就是 `correct()` 之后
//! 的那份，与 `open_*_edit` 里重新 `parse_*` 的输入同源。

use std::rc::Rc;

use slint::ComponentHandle;

use crate::settings_writer::SettingsWriter;
use crate::{AppWindow, category};

/// 把九个「开合」回调装到窗口上。
///
/// 每个闭包各持一份 `Weak<AppWindow>` 与 `Rc<SettingsWriter>`：`window.run()` 期间
/// 回调活到窗口销毁之后，强引用会成环。
pub fn install(window: &AppWindow, writer: Rc<SettingsWriter>) {
    let win = window.as_weak();
    let open = Rc::clone(&writer);
    window.on_headersPresetAddRequested(move || {
        if let Some(window) = win.upgrade() {
            open_headers(&window, &open, None);
        }
    });
    let win = window.as_weak();
    let open = Rc::clone(&writer);
    window.on_headersPresetEditRequested(move |index| {
        if let Some(window) = win.upgrade() {
            open_headers(&window, &open, Some(index as i64));
        }
    });
    let win = window.as_weak();
    window.on_headersPresetEditClosed(move || {
        if let Some(window) = win.upgrade() {
            close_headers(&window);
        }
    });

    let win = window.as_weak();
    let open = Rc::clone(&writer);
    window.on_identityPresetAddRequested(move || {
        if let Some(window) = win.upgrade() {
            open_identity(&window, &open, None);
        }
    });
    let win = window.as_weak();
    let open = Rc::clone(&writer);
    window.on_identityPresetEditRequested(move |index| {
        if let Some(window) = win.upgrade() {
            open_identity(&window, &open, Some(index as i64));
        }
    });
    let win = window.as_weak();
    window.on_identityPresetEditClosed(move || {
        if let Some(window) = win.upgrade() {
            close_identity(&window);
        }
    });

    let win = window.as_weak();
    window.on_categoryAddRequested(move || {
        if let Some(window) = win.upgrade() {
            open_category_new(&window);
        }
    });
    let win = window.as_weak();
    let open = Rc::clone(&writer);
    window.on_categoryEditRequested(move |id| {
        if let Some(window) = win.upgrade() {
            open_category(&window, &open, &id);
        }
    });
    let win = window.as_weak();
    window.on_categoryEditClosed(move || {
        if let Some(window) = win.upgrade() {
            close_category(&window);
        }
    });
}

/// 打开标头预设覆盖层：`index` `None` = 添加（名称留空、标头回显当前选中的预设，
/// 镜像 `_onAddClicked` 的 `preset={"name": "", "headers": currentHeaders()}`），
/// `Some(i)` = 编辑回显 `presets[i]`。
pub fn open_headers(window: &AppWindow, writer: &SettingsWriter, index: Option<i64>) {
    let Some(index) = checked(index, list_len(writer, "headersPresets")) else {
        return;
    };
    let config = writer.config();
    crate::open_headers_preset_edit(window, &config, index);
}

/// 打开身份预设覆盖层（`None` = 添加：名称/hosts/UA 空、TLS 指纹落「跟随全局默认」）。
pub fn open_identity(window: &AppWindow, writer: &SettingsWriter, index: Option<i64>) {
    let Some(index) = checked(index, list_len(writer, "identityPresets")) else {
        return;
    };
    let config = writer.config();
    crate::open_identity_preset_edit(window, &config, index);
}

/// 打开分类表单的添加态（镜像 `CategoryEditDialog(category=None)`：字段全空、icon 落 0）。
pub fn open_category_new(window: &AppWindow) {
    crate::open_category_edit(window, None);
}

/// 打开分类表单的编辑态：按 `categoryId` 查行，查不到就什么都不做
/// （`category_settings.py:137-138` 的 `if category is None: return`）。
///
/// 分类表走 [`SettingsWriter::categories`]，与 `CategoryService._load` 同口径 ——
/// `categoryRules` 为空时是八预设，不是空表。
pub fn open_category(window: &AppWindow, writer: &SettingsWriter, category_id: &str) {
    let rules = writer.categories();
    let Some(rule) = category::category_by_id(&rules, category_id) else {
        return;
    };
    crate::open_category_edit(window, Some(rule));
}

pub fn close_headers(window: &AppWindow) {
    window.set_headersEditVisible(false);
}

pub fn close_identity(window: &AppWindow) {
    window.set_identityEditVisible(false);
}

pub fn close_category(window: &AppWindow) {
    window.set_categoryEditVisible(false);
}

/// 属性名 → 列表长度（缺键/非数组 → 0，与 `SettingsWriter::list` 的容错同口径）。
fn list_len(writer: &SettingsWriter, attr: &str) -> usize {
    match writer.get(attr) {
        Some(serde_json::Value::Array(items)) => items.len(),
        _ => 0,
    }
}

/// 索引守卫：`None`（添加）直接放行；`Some(i)` 越界 → `None` 表示「别开」。
fn checked(index: Option<i64>, len: usize) -> Option<Option<usize>> {
    match index {
        None => Some(None),
        Some(i) if i < 0 || i as u64 >= len as u64 => None,
        Some(i) => Some(Some(i as usize)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use gd3_core::config::UserConfig;

    use super::*;
    use crate::config_store::ConfigStore;
    use crate::{headers, identity};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-settings-dialogs-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一个「路径在临时目录里、内存是缺省值」的 writer + 装好九个回调的窗口。
    fn window(tag: &str) -> (Rc<SettingsWriter>, AppWindow) {
        let dir = temp_dir(tag);
        let path = dir.join("UserConfig.json");
        let writer = Rc::new(SettingsWriter::new(ConfigStore::new(&path)));
        let window = crate::create_headless();
        install(&window, Rc::clone(&writer));
        (writer, window)
    }

    /// 缺省配置里的标头预设表（`Network/HeadersPresets` 缺省一项「默认」）。
    fn default_header_presets() -> Vec<headers::HeaderPreset> {
        let config = UserConfig::new();
        headers::parse_presets(config.get_str("Network", "HeadersPresets").unwrap_or(""))
    }

    #[test]
    fn adding_a_headers_preset_opens_the_overlay_with_the_current_headers() {
        let (writer, window) = window("headers-add");
        assert_eq!(list_len(&writer, "headersPresets"), 1);

        window.invoke_headersPresetAddRequested();
        assert!(window.get_headersEditVisible());
        assert_eq!(window.get_headersEditIndex(), -1, "-1 表示添加态");
        assert_eq!(window.get_headersEditName(), "", "添加态名称留空");
        let presets = default_header_presets();
        assert_eq!(
            window.get_headersEditPairs(),
            headers::headers_to_text(&presets[0].headers),
            "标头回显 currentHeaders()"
        );
        assert_eq!(window.get_headersEditDuplicateWarning(), "");
    }

    #[test]
    fn editing_a_headers_preset_echoes_it() {
        let (_writer, window) = window("headers-edit");
        window.invoke_headersPresetEditRequested(0);
        assert!(window.get_headersEditVisible());
        assert_eq!(window.get_headersEditIndex(), 0);
        assert_eq!(window.get_headersEditName(), "默认");
        assert!(!window.get_headersEditPairs().is_empty());
    }

    #[test]
    fn an_out_of_range_headers_index_keeps_the_overlay_closed() {
        let (_writer, window) = window("headers-range");
        window.invoke_headersPresetEditRequested(9);
        assert!(!window.get_headersEditVisible(), "越界索引不开覆盖层");
        assert_eq!(window.get_headersEditIndex(), -1, "窗口属性一个都没动");
    }

    #[test]
    fn closing_the_headers_overlay_hides_it() {
        let (_writer, window) = window("headers-close");
        window.invoke_headersPresetEditRequested(0);
        assert!(window.get_headersEditVisible());
        window.invoke_headersPresetEditClosed();
        assert!(!window.get_headersEditVisible());
    }

    #[test]
    fn adding_an_identity_preset_opens_the_empty_form() {
        let (_writer, window) = window("identity-add");
        window.invoke_identityPresetAddRequested();
        assert!(window.get_identityEditVisible());
        assert_eq!(window.get_identityEditIndex(), -1);
        assert_eq!(window.get_identityEditName(), "");
        assert_eq!(window.get_identityEditHosts(), "");
        assert_eq!(window.get_identityEditUserAgent(), "");
        assert_eq!(
            window.get_identityEditProfileIndex(),
            0,
            "添加态 TLS 指纹落「跟随全局默认」"
        );
        assert!(!window.get_identityEditHostsError());
    }

    #[test]
    fn editing_an_identity_preset_echoes_it() {
        let (writer, window) = window("identity-edit");
        // 缺省 `identityPresets` 已有一项（`Network/IdentityPresets` 的缺省
        // 「百度网盘客户端」），先经公开动作追加一项。
        let actions = writer.actions();
        (actions.identity_preset_edit_submit)((
            -1,
            identity::IdentityPreset {
                name: "我的预设".into(),
                client_profile: "raw".into(),
                user_agent: "UA".into(),
                hosts: vec!["a.com".into()],
                is_enabled: true,
            },
        ));
        let count = list_len(&writer, "identityPresets");
        assert_eq!(count, 2, "缺省一项 + 追加一项");
        assert!(!window.get_identityEditVisible(), "追加不碰窗口");

        window.invoke_identityPresetEditRequested((count - 1) as i32);
        assert!(window.get_identityEditVisible());
        assert_eq!(window.get_identityEditIndex() as usize, count - 1);
        assert_eq!(window.get_identityEditName(), "我的预设");
        assert_eq!(window.get_identityEditHosts(), "a.com");
        assert_eq!(window.get_identityEditUserAgent(), "UA");
    }

    #[test]
    fn an_out_of_range_identity_index_keeps_the_overlay_closed() {
        let (_writer, window) = window("identity-range");
        window.invoke_identityPresetEditRequested(5);
        assert!(
            !window.get_identityEditVisible(),
            "空表上编辑任何索引都不开"
        );
        assert_eq!(window.get_identityEditIndex(), -1);
    }

    #[test]
    fn closing_the_identity_overlay_hides_it() {
        let (_writer, window) = window("identity-close");
        window.invoke_identityPresetAddRequested();
        assert!(window.get_identityEditVisible());
        window.invoke_identityPresetEditClosed();
        assert!(!window.get_identityEditVisible());
    }

    #[test]
    fn adding_a_category_opens_the_empty_form() {
        let (_writer, window) = window("category-add");
        window.invoke_categoryAddRequested();
        assert!(window.get_categoryEditVisible());
        assert_eq!(window.get_categoryEditId(), "", "空 id = 添加态");
        assert_eq!(window.get_categoryEditName(), "");
        assert_eq!(window.get_categoryEditExtensions(), "");
        assert_eq!(window.get_categoryEditIconIndex(), 0);
    }

    #[test]
    fn editing_a_category_echoes_the_matching_rule() {
        let (writer, window) = window("category-edit");
        let rules = writer.categories();
        let rule = category::category_by_id(&rules, "cat_video").expect("八预设里有 cat_video");

        window.invoke_categoryEditRequested("cat_video".into());
        assert!(window.get_categoryEditVisible());
        assert_eq!(window.get_categoryEditId(), "cat_video");
        assert_eq!(window.get_categoryEditName(), rule.name);
        assert_eq!(
            window.get_categoryEditExtensions(),
            rule.extensions.join(","),
            "扩展名逐项逗号连接"
        );
        assert_eq!(window.get_categoryEditFolder(), "{default}/Video");
    }

    #[test]
    fn an_unknown_category_id_keeps_the_overlay_closed() {
        let (_writer, window) = window("category-miss");
        window.invoke_categoryEditRequested("cat_nope".into());
        assert!(!window.get_categoryEditVisible(), "查不到就什么都不做");
        assert_eq!(window.get_categoryEditId(), "");
    }

    #[test]
    fn closing_the_category_overlay_hides_it() {
        let (_writer, window) = window("category-close");
        window.invoke_categoryAddRequested();
        assert!(window.get_categoryEditVisible());
        window.invoke_categoryEditClosed();
        assert!(!window.get_categoryEditVisible());
    }
}
