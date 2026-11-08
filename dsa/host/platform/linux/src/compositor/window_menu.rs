//! A task window's menu, with no Wayland in it.
//!
//! A Wayland window cannot add items to the desktop's own window menu, so the window draws a
//! menu of its own, in a popup (`xdg_popup`). It opens from the icon's cell and from a right
//! click on the title bar, where the desktop's window menu would open. It carries the window's
//! own items (Minimize, Maximize or Restore, Close), then, after a separator, the items an
//! Android desktop window offers in its window menu: New window (only for an app that supports
//! several windows), Screenshot, Aspect ratio, Open by default and App info, as the Windows
//! host puts them in the system menu.
//!
//! The window takes the screenshot itself, since it has the window's frames. The other
//! Android items go to the window rules as `FromHost::MenuChosen` and become requests to the
//! guest ([`menu_work`]), or open DSA Settings at the app's page.
//!
//! The items speak Korean when the session reads Korean, else English.

use dsa_host_core::compositor::caption::scaled;
use dsa_host_core::compositor::window_hosts::{MenuAction, MenuWork};
use dsa_host_core::tasks::{AppSettingsPage, TaskRequest};

use super::title_bar::Coverage;

/// One entry of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuItem {
    Minimize,
    Maximize,
    Restore,
    Close,
    /// The line between the window's own items and Android's.
    Separator,
    NewWindow,
    Screenshot,
    AspectRatio,
    OpenByDefault,
    AppInfo,
}

/// The menu's entries in order: the window's own items (Restore in place of Maximize while
/// the window is maximized), a separator, then Android's. New window is left out for an app
/// that does not support several windows, since its second task would only bring the first to
/// the front.
pub fn items(maximized: bool, new_window: bool) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::Minimize,
        if maximized {
            MenuItem::Restore
        } else {
            MenuItem::Maximize
        },
        MenuItem::Close,
        MenuItem::Separator,
    ];
    if new_window {
        items.push(MenuItem::NewWindow);
    }
    items.extend([
        MenuItem::Screenshot,
        MenuItem::AspectRatio,
        MenuItem::OpenByDefault,
        MenuItem::AppInfo,
    ]);
    items
}

/// What the window rules do for an entry; `None` for the window's own items, the separator
/// and Screenshot, which the window carries out itself.
pub fn action(item: MenuItem) -> Option<MenuAction> {
    match item {
        MenuItem::NewWindow => Some(MenuAction::NewWindow),
        MenuItem::AspectRatio => Some(MenuAction::AspectRatio),
        MenuItem::OpenByDefault => Some(MenuAction::OpenByDefault),
        MenuItem::AppInfo => Some(MenuAction::AppInfo),
        _ => None,
    }
}

/// What the window manager does for `action`, chosen in a window of the app `package`; `None`
/// for New window of an app that does not support several windows, which its menu never
/// offers.
pub fn menu_work(action: MenuAction, package: &str, multiple_windows: bool) -> Option<MenuWork> {
    let settings = |page| {
        MenuWork::Request(TaskRequest::OpenAppSettings {
            package_name: package.to_string(),
            page,
        })
    };
    Some(match action {
        MenuAction::NewWindow if multiple_windows => MenuWork::Request(TaskRequest::NewWindow {
            package_name: package.to_string(),
        }),
        MenuAction::NewWindow => return None,
        MenuAction::AspectRatio => settings(AppSettingsPage::AspectRatio),
        MenuAction::OpenByDefault => settings(AppSettingsPage::OpenByDefault),
        MenuAction::AppInfo => MenuWork::AppPage {
            package: package.to_string(),
        },
    })
}

/// The arguments that open DSA Settings on the Apps page with `package`'s row open.
pub fn app_page_arguments(package: &str) -> [String; 4] {
    [
        "--page".into(),
        "apps".into(),
        "--app".into(),
        package.into(),
    ]
}

/// The menu's words in one language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuText {
    pub minimize: &'static str,
    pub maximize: &'static str,
    pub restore: &'static str,
    pub close: &'static str,
    pub new_window: &'static str,
    pub screenshot: &'static str,
    pub aspect_ratio: &'static str,
    pub open_by_default: &'static str,
    pub app_info: &'static str,
    /// What a screenshot's file name starts with.
    pub screenshot_file: &'static str,
}

/// The menu's words: Korean for a session that reads Korean, else English.
pub fn menu_text(korean: bool) -> MenuText {
    if korean {
        MenuText {
            minimize: "최소화",
            maximize: "최대화",
            restore: "복원",
            close: "닫기",
            new_window: "새 창",
            screenshot: "스크린샷",
            aspect_ratio: "화면 비율",
            open_by_default: "링크 열기 설정",
            app_info: "앱 정보",
            screenshot_file: "스크린샷",
        }
    } else {
        MenuText {
            minimize: "Minimize",
            maximize: "Maximize",
            restore: "Restore",
            close: "Close",
            new_window: "New window",
            screenshot: "Screenshot",
            aspect_ratio: "Aspect ratio",
            open_by_default: "Open by default",
            app_info: "App info",
            screenshot_file: "Screenshot",
        }
    }
}

/// The label of an entry; `None` for the separator.
pub fn label(text: &MenuText, item: MenuItem) -> Option<&'static str> {
    Some(match item {
        MenuItem::Separator => return None,
        MenuItem::Minimize => text.minimize,
        MenuItem::Maximize => text.maximize,
        MenuItem::Restore => text.restore,
        MenuItem::Close => text.close,
        MenuItem::NewWindow => text.new_window,
        MenuItem::Screenshot => text.screenshot,
        MenuItem::AspectRatio => text.aspect_ratio,
        MenuItem::OpenByDefault => text.open_by_default,
        MenuItem::AppInfo => text.app_info,
    })
}

/// Whether a session reads Korean: the language gettext would choose its messages in, from
/// `LC_ALL`, `LC_MESSAGES` or `LANG`, the first that is set, as `var` reads them.
pub fn reads_korean_in(var: impl Fn(&str) -> Option<String>) -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| var(name).filter(|value| !value.is_empty()))
        .is_some_and(|locale| locale.starts_with("ko"))
}

/// The height of an item at 100%, in pixels.
const ITEM_AT_100: u32 = 32;
/// The height of the separator's row at 100%.
const SEPARATOR_AT_100: u32 = 9;
/// The space around the items, and before and after each label, at 100%.
const PADDING_AT_100: u32 = 4;
const LABEL_PADDING_AT_100: u32 = 12;
/// The narrowest a menu is at 100%.
const MIN_WIDTH_AT_100: u32 = 180;
/// The labels' text size at 100%, in pixels.
pub const TEXT_AT_100: u32 = 14;

/// The menu's colors, 0xAARRGGBB: a light menu, as the desktop's own menus are by default.
const BACKGROUND: u32 = 0xFFF9_F9F9;
const BORDER: u32 = 0x33_000000;
const TEXT: u32 = 0xE4_000000;
const HIGHLIGHT: u32 = 0x14_000000;
const SEPARATOR: u32 = 0x1F_000000;

/// Where each entry of a menu is, in its buffer's pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLayout {
    pub items: Vec<MenuItem>,
    pub width: u32,
    pub height: u32,
    /// Each entry's top and height, in order.
    pub rows: Vec<(u32, u32)>,
    /// Where the labels start from the left.
    pub label_left: u32,
}

impl MenuLayout {
    /// The layout of `items` at `dpi`, wide enough for the widest of `label_widths` (one per
    /// item, in pixels at that DPI).
    pub fn new(items: Vec<MenuItem>, label_widths: &[u32], dpi: u32) -> MenuLayout {
        let padding = scaled(PADDING_AT_100, dpi);
        let label_padding = scaled(LABEL_PADDING_AT_100, dpi);
        let widest = label_widths.iter().copied().max().unwrap_or(0);
        // A whole number of logical units wide, as a buffer of a scaled surface must be.
        let unit = (dpi / 96).max(1);
        let width = (widest + 2 * (padding + label_padding))
            .max(scaled(MIN_WIDTH_AT_100, dpi))
            .div_ceil(unit)
            * unit;
        let mut rows = Vec::with_capacity(items.len());
        let mut top = padding;
        for item in &items {
            let height = if *item == MenuItem::Separator {
                scaled(SEPARATOR_AT_100, dpi)
            } else {
                scaled(ITEM_AT_100, dpi)
            };
            rows.push((top, height));
            top += height;
        }
        MenuLayout {
            items,
            width,
            height: top + padding,
            rows,
            label_left: padding + label_padding,
        }
    }

    /// The entry under the point (`x`, `y`) that can be chosen; `None` off the entries and on
    /// the separator.
    pub fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        if x < 0.0 || x >= f64::from(self.width) {
            return None;
        }
        self.rows
            .iter()
            .position(|(top, height)| y >= f64::from(*top) && y < f64::from(top + height))
            .filter(|&index| self.items[index] != MenuItem::Separator)
    }

    /// The entry the keyboard moves to from `from`, down or up, past the separator, wrapping
    /// round at the ends.
    pub fn step(&self, from: Option<usize>, down: bool) -> Option<usize> {
        let count = self.items.len();
        if count == 0 {
            return None;
        }
        let mut at = match (from, down) {
            (None, true) => count - 1,
            (None, false) => 0,
            (Some(at), _) => at,
        };
        for _ in 0..count {
            at = if down {
                (at + 1) % count
            } else {
                (at + count - 1) % count
            };
            if self.items[at] != MenuItem::Separator {
                return Some(at);
            }
        }
        None
    }
}

fn premultiplied(color: u32, coverage: f64) -> [f64; 4] {
    let alpha = f64::from(color >> 24) / 255.0 * coverage;
    [
        f64::from(color & 0xff) / 255.0 * alpha,
        f64::from((color >> 8) & 0xff) / 255.0 * alpha,
        f64::from((color >> 16) & 0xff) / 255.0 * alpha,
        alpha,
    ]
}

/// `top` over the premultiplied BGRA pixel `pixel`.
fn blend(pixel: &mut [u8], top: [f64; 4]) {
    let keep = 1.0 - top[3];
    for (channel, value) in pixel.iter_mut().zip(top) {
        *channel = (f64::from(*channel) * keep + value * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

/// The menu as premultiplied BGRA rows (`ARGB8888`), `layout.width` x `layout.height`: the
/// background with a border, the highlighted entry, the separator and each label (one per
/// item, `None` for the separator), centred top to bottom in its row.
pub fn paint_menu(
    layout: &MenuLayout,
    labels: &[Option<Coverage>],
    highlighted: Option<usize>,
    dpi: u32,
) -> Vec<u8> {
    let (width, height) = (layout.width as usize, layout.height as usize);
    let mut pixels = vec![0u8; width * height * 4];
    let border = scaled(1, dpi).max(1) as usize;
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            let edge = x < border || y < border || x >= width - border || y >= height - border;
            let color = if edge { BORDER } else { BACKGROUND };
            blend(&mut pixels[at..at + 4], premultiplied(color, 1.0));
        }
    }
    let padding = scaled(PADDING_AT_100, dpi) as usize;
    for (index, (item, (top, row_height))) in layout.items.iter().zip(&layout.rows).enumerate() {
        let (top, row_height) = (*top as usize, *row_height as usize);
        if *item == MenuItem::Separator {
            let y = top + row_height / 2;
            for x in padding..width.saturating_sub(padding) {
                let at = (y * width + x) * 4;
                blend(&mut pixels[at..at + 4], premultiplied(SEPARATOR, 1.0));
            }
            continue;
        }
        if highlighted == Some(index) {
            for y in top..(top + row_height).min(height) {
                for x in padding..width.saturating_sub(padding) {
                    let at = (y * width + x) * 4;
                    blend(&mut pixels[at..at + 4], premultiplied(HIGHLIGHT, 1.0));
                }
            }
        }
        let Some(Some(text)) = labels.get(index) else {
            continue;
        };
        let text_top = top as i64 + (row_height as i64 - i64::from(text.height)) / 2;
        let left = layout.label_left as usize;
        for row in 0..text.height as usize {
            let y = text_top + row as i64;
            if y < 0 || y as usize >= height {
                continue;
            }
            for column in 0..text.width as usize {
                let x = left + column;
                if x >= width.saturating_sub(padding) {
                    break;
                }
                let coverage = text.alpha[row * text.width as usize + column];
                if coverage == 0 {
                    continue;
                }
                let at = (y as usize * width + x) * 4;
                blend(
                    &mut pixels[at..at + 4],
                    premultiplied(TEXT, f64::from(coverage) / 255.0),
                );
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr22_the_window_menu_carries_the_android_window_menu() {
        use MenuItem::*;
        assert_eq!(
            items(false, true),
            vec![
                Minimize,
                Maximize,
                Close,
                Separator,
                NewWindow,
                Screenshot,
                AspectRatio,
                OpenByDefault,
                AppInfo
            ]
        );
        // New window only for an app that supports several windows; Restore while maximized.
        assert_eq!(
            items(true, false),
            vec![
                Minimize,
                Restore,
                Close,
                Separator,
                Screenshot,
                AspectRatio,
                OpenByDefault,
                AppInfo
            ]
        );
        assert_eq!(action(NewWindow), Some(MenuAction::NewWindow));
        assert_eq!(action(AspectRatio), Some(MenuAction::AspectRatio));
        assert_eq!(action(OpenByDefault), Some(MenuAction::OpenByDefault));
        assert_eq!(action(AppInfo), Some(MenuAction::AppInfo));
        for item in [Screenshot, Minimize, Maximize, Restore, Close, Separator] {
            assert_eq!(action(item), None, "{item:?}");
        }
    }

    #[test]
    fn fr22_each_window_menu_item_does_what_android_s_does() {
        let package = "com.example.notes";
        assert_eq!(
            menu_work(MenuAction::NewWindow, package, true),
            Some(MenuWork::Request(TaskRequest::NewWindow {
                package_name: package.into()
            }))
        );
        assert_eq!(menu_work(MenuAction::NewWindow, package, false), None);
        assert_eq!(
            menu_work(MenuAction::AspectRatio, package, false),
            Some(MenuWork::Request(TaskRequest::OpenAppSettings {
                package_name: package.into(),
                page: AppSettingsPage::AspectRatio
            }))
        );
        assert_eq!(
            menu_work(MenuAction::OpenByDefault, package, false),
            Some(MenuWork::Request(TaskRequest::OpenAppSettings {
                package_name: package.into(),
                page: AppSettingsPage::OpenByDefault
            }))
        );
        assert_eq!(
            menu_work(MenuAction::AppInfo, package, false),
            Some(MenuWork::AppPage {
                package: package.into()
            })
        );
        assert_eq!(
            app_page_arguments(package),
            ["--page", "apps", "--app", package].map(String::from)
        );
    }

    #[test]
    fn fr22_the_window_menu_speaks_korean_and_english() {
        for korean in [false, true] {
            let text = menu_text(korean);
            let labels: Vec<&str> = items(false, true)
                .into_iter()
                .filter_map(|item| label(&text, item))
                .collect();
            assert_eq!(labels.len(), 8);
            assert!(labels.iter().all(|label| !label.is_empty()));
        }
        assert_eq!(menu_text(false).new_window, "New window");
        assert_eq!(menu_text(true).app_info, "앱 정보");
        assert_eq!(menu_text(true).screenshot_file, "스크린샷");
        let korean = |name: &str| (name == "LANG").then(|| "ko_KR.UTF-8".to_string());
        let english = |name: &str| (name == "LANG").then(|| "en_US.UTF-8".to_string());
        // LC_ALL comes first, as gettext reads it.
        let both = |name: &str| match name {
            "LC_ALL" => Some("en_GB.UTF-8".to_string()),
            "LANG" => Some("ko_KR.UTF-8".to_string()),
            _ => None,
        };
        assert!(reads_korean_in(korean));
        assert!(!reads_korean_in(english));
        assert!(!reads_korean_in(both));
    }

    #[test]
    fn fr22_the_window_menu_is_chosen_by_pointer_and_keyboard() {
        let layout = MenuLayout::new(items(false, false), &[60; 8], 96);
        assert_eq!(layout.width, 180);
        // Padding 4, then three 32 px items, the 9 px separator, four items, padding 4.
        assert_eq!(layout.height, 4 + 3 * 32 + 9 + 4 * 32 + 4);
        assert_eq!(layout.item_at(10.0, 5.0), Some(0));
        assert_eq!(layout.item_at(10.0, 4.0 + 3.0 * 32.0 + 4.0), None);
        assert_eq!(layout.item_at(10.0, 4.0 + 3.0 * 32.0 + 9.0), Some(4));
        assert_eq!(layout.item_at(-1.0, 5.0), None);
        assert_eq!(layout.item_at(10.0, 1000.0), None);
        // The keyboard skips the separator and wraps round.
        assert_eq!(layout.step(None, true), Some(0));
        assert_eq!(layout.step(Some(2), true), Some(4));
        assert_eq!(layout.step(Some(4), false), Some(2));
        assert_eq!(layout.step(Some(0), false), Some(7));
        // A long label widens the menu, and the menu scales.
        assert_eq!(
            MenuLayout::new(items(false, false), &[300], 96).width,
            300 + 32
        );
        assert_eq!(
            MenuLayout::new(items(false, false), &[60; 8], 192).height,
            2 * layout.height
        );
    }

    #[test]
    fn fr22_the_window_menu_draws_its_labels_and_highlight() {
        let layout = MenuLayout::new(vec![MenuItem::Screenshot, MenuItem::AppInfo], &[4], 96);
        let label = Coverage {
            width: 4,
            height: 2,
            alpha: vec![0xff; 8],
        };
        let labels = vec![Some(label.clone()), Some(label)];
        let plain = paint_menu(&layout, &labels, None, 96);
        let lit = paint_menu(&layout, &labels, Some(1), 96);
        let width = layout.width as usize;
        let pixel = |pixels: &[u8], x: usize, y: usize| {
            let at = (y * width + x) * 4;
            pixels[at..at + 4].to_vec()
        };
        // The background is opaque, and the highlight darkens the second row only.
        let (first_top, _) = layout.rows[0];
        let (second_top, _) = layout.rows[1];
        assert_eq!(pixel(&plain, 6, first_top as usize + 2)[3], 0xff);
        assert_eq!(
            pixel(&plain, 6, first_top as usize + 2),
            pixel(&lit, 6, first_top as usize + 2)
        );
        assert_ne!(
            pixel(&plain, 6, second_top as usize + 2),
            pixel(&lit, 6, second_top as usize + 2)
        );
        // The label is drawn dark at its place, centred in its row.
        let y = first_top as usize + (32 - 2) / 2;
        let text = pixel(&plain, layout.label_left as usize, y);
        assert!(text[0] < 0x40, "{text:?}");
    }
}
