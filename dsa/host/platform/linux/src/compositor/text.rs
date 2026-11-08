//! Text for the title bar's name and the window menu's labels, laid out and drawn by Pango
//! with cairo, which every desktop that runs GTK programs has. They are loaded at run time,
//! so building the host needs no development package, and a desktop without them still shows
//! its windows: the title bar then has no name, and the window menu falls back to the
//! desktop's own.
//!
//! Text is drawn as coverage (one alpha byte a pixel, grayscale antialiased), which the
//! caller tints with the glyphs' color, in the desktop's interface font (GNOME's
//! `font-name` setting when it has one) at an absolute pixel size, so it is as sharp at 200%
//! as at 100%. Pango falls back to other fonts for characters the font lacks, Hangul among
//! them.

use std::ffi::{c_char, c_int, c_void, CString};

use libloading::Library;

use super::title_bar::Coverage;

type Surface = c_void;
type Cairo = c_void;
type Layout = c_void;
type FontDescription = c_void;

const CAIRO_FORMAT_A8: c_int = 2;
const PANGO_SCALE: f64 = 1024.0;
const PANGO_ELLIPSIZE_END: c_int = 3;

/// The longest side drawn, in pixels; text never needs more.
const MAX_SIDE: c_int = 4096;

struct Api {
    _cairo: Library,
    _pango: Library,
    _pango_cairo: Library,
    _gobject: Library,
    image_surface_create: unsafe extern "C" fn(c_int, c_int, c_int) -> *mut Surface,
    surface_status: unsafe extern "C" fn(*mut Surface) -> c_int,
    surface_flush: unsafe extern "C" fn(*mut Surface),
    surface_destroy: unsafe extern "C" fn(*mut Surface),
    image_surface_get_data: unsafe extern "C" fn(*mut Surface) -> *mut u8,
    image_surface_get_stride: unsafe extern "C" fn(*mut Surface) -> c_int,
    create: unsafe extern "C" fn(*mut Surface) -> *mut Cairo,
    destroy: unsafe extern "C" fn(*mut Cairo),
    set_source_rgba: unsafe extern "C" fn(*mut Cairo, f64, f64, f64, f64),
    move_to: unsafe extern "C" fn(*mut Cairo, f64, f64),
    create_layout: unsafe extern "C" fn(*mut Cairo) -> *mut Layout,
    show_layout: unsafe extern "C" fn(*mut Cairo, *mut Layout),
    font_description_from_string: unsafe extern "C" fn(*const c_char) -> *mut FontDescription,
    font_description_set_absolute_size: unsafe extern "C" fn(*mut FontDescription, f64),
    font_description_free: unsafe extern "C" fn(*mut FontDescription),
    layout_set_font_description: unsafe extern "C" fn(*mut Layout, *const FontDescription),
    layout_set_text: unsafe extern "C" fn(*mut Layout, *const c_char, c_int),
    layout_set_width: unsafe extern "C" fn(*mut Layout, c_int),
    layout_set_ellipsize: unsafe extern "C" fn(*mut Layout, c_int),
    layout_get_pixel_size: unsafe extern "C" fn(*mut Layout, *mut c_int, *mut c_int),
    object_unref: unsafe extern "C" fn(*mut c_void),
}

fn open(name: &str) -> Result<Library, String> {
    // SAFETY: cairo, Pango and GObject run no code of their own that matters when loaded.
    unsafe { Library::new(name) }.map_err(|error| format!("cannot load {name}: {error}"))
}

impl Api {
    fn load() -> Result<Api, String> {
        let cairo = open("libcairo.so.2")?;
        let gobject = open("libgobject-2.0.so.0")?;
        let pango = open("libpango-1.0.so.0")?;
        let pango_cairo = open("libpangocairo-1.0.so.0")?;
        // SAFETY: each symbol is looked up with the type its library's header gives it.
        unsafe {
            macro_rules! symbol {
                ($library:ident, $name:literal) => {
                    *$library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|error| format!("{} is missing: {error}", $name))?
                };
            }
            Ok(Api {
                image_surface_create: symbol!(cairo, "cairo_image_surface_create"),
                surface_status: symbol!(cairo, "cairo_surface_status"),
                surface_flush: symbol!(cairo, "cairo_surface_flush"),
                surface_destroy: symbol!(cairo, "cairo_surface_destroy"),
                image_surface_get_data: symbol!(cairo, "cairo_image_surface_get_data"),
                image_surface_get_stride: symbol!(cairo, "cairo_image_surface_get_stride"),
                create: symbol!(cairo, "cairo_create"),
                destroy: symbol!(cairo, "cairo_destroy"),
                set_source_rgba: symbol!(cairo, "cairo_set_source_rgba"),
                move_to: symbol!(cairo, "cairo_move_to"),
                create_layout: symbol!(pango_cairo, "pango_cairo_create_layout"),
                show_layout: symbol!(pango_cairo, "pango_cairo_show_layout"),
                font_description_from_string: symbol!(pango, "pango_font_description_from_string"),
                font_description_set_absolute_size: symbol!(
                    pango,
                    "pango_font_description_set_absolute_size"
                ),
                font_description_free: symbol!(pango, "pango_font_description_free"),
                layout_set_font_description: symbol!(pango, "pango_layout_set_font_description"),
                layout_set_text: symbol!(pango, "pango_layout_set_text"),
                layout_set_width: symbol!(pango, "pango_layout_set_width"),
                layout_set_ellipsize: symbol!(pango, "pango_layout_set_ellipsize"),
                layout_get_pixel_size: symbol!(pango, "pango_layout_get_pixel_size"),
                object_unref: symbol!(gobject, "g_object_unref"),
                _cairo: cairo,
                _pango: pango,
                _pango_cairo: pango_cairo,
                _gobject: gobject,
            })
        }
    }
}

/// The family of a GNOME `font-name` value, as `gsettings get` prints it: `'Cantarell 11'`
/// is Cantarell. `None` for an empty one.
pub fn family_of_font_name(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('\'').trim();
    let family = match value.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f64>().is_ok() => family.trim(),
        _ => value,
    };
    (!family.is_empty()).then(|| family.to_string())
}

/// The desktop's interface font family: GNOME's `font-name` setting, else `Sans`, which
/// fontconfig turns into the system's sans-serif font.
fn interface_family() -> String {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "font-name"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| family_of_font_name(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_else(|| "Sans".to_string())
}

/// Draws lines of text.
pub struct Text {
    api: Api,
    family: CString,
}

impl Text {
    pub fn load() -> Result<Text, String> {
        let api = Api::load()?;
        let family = CString::new(interface_family()).unwrap_or_else(|_| c"Sans".to_owned());
        Ok(Text { api, family })
    }

    /// A layout of `text` at `px` pixels, cut with an ellipsis at `max_width` pixels, on `cr`.
    ///
    /// # Safety
    ///
    /// `cr` is a live cairo context; the layout returned is unreferenced by the caller.
    unsafe fn layout(
        &self,
        cr: *mut Cairo,
        text: &str,
        px: f64,
        max_width: Option<u32>,
    ) -> *mut Layout {
        let api = &self.api;
        let layout = (api.create_layout)(cr);
        if layout.is_null() {
            return layout;
        }
        let font = (api.font_description_from_string)(self.family.as_ptr());
        if !font.is_null() {
            (api.font_description_set_absolute_size)(font, px * PANGO_SCALE);
            (api.layout_set_font_description)(layout, font);
            (api.font_description_free)(font);
        }
        (api.layout_set_text)(layout, text.as_ptr().cast(), text.len() as c_int);
        if let Some(width) = max_width {
            (api.layout_set_width)(layout, (f64::from(width) * PANGO_SCALE) as c_int);
            (api.layout_set_ellipsize)(layout, PANGO_ELLIPSIZE_END);
        }
        layout
    }

    /// `text` on one line at `px` pixels (the font's em size), cut with an ellipsis so it is
    /// at most `max_width` pixels wide, as coverage. `None` for empty text, or when cairo
    /// cannot draw.
    pub fn line(&self, text: &str, px: f64, max_width: Option<u32>) -> Option<Coverage> {
        let text: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if text.is_empty() || max_width == Some(0) {
            return None;
        }
        let api = &self.api;
        // SAFETY: every object made here is destroyed here, and the pixels are read while
        // their surface lives.
        unsafe {
            let (mut width, mut height) = (0, 0);
            let scratch = (api.image_surface_create)(CAIRO_FORMAT_A8, 1, 1);
            if (api.surface_status)(scratch) != 0 {
                (api.surface_destroy)(scratch);
                return None;
            }
            let cr = (api.create)(scratch);
            let layout = self.layout(cr, &text, px, max_width);
            if !layout.is_null() {
                (api.layout_get_pixel_size)(layout, &mut width, &mut height);
                (api.object_unref)(layout);
            }
            (api.destroy)(cr);
            (api.surface_destroy)(scratch);
            if let Some(max) = max_width {
                width = width.min(max as c_int);
            }
            if width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE {
                return None;
            }

            let surface = (api.image_surface_create)(CAIRO_FORMAT_A8, width, height);
            if (api.surface_status)(surface) != 0 {
                (api.surface_destroy)(surface);
                return None;
            }
            let cr = (api.create)(surface);
            let layout = self.layout(cr, &text, px, max_width);
            if !layout.is_null() {
                (api.set_source_rgba)(cr, 0.0, 0.0, 0.0, 1.0);
                (api.move_to)(cr, 0.0, 0.0);
                (api.show_layout)(cr, layout);
                (api.object_unref)(layout);
            }
            (api.destroy)(cr);
            (api.surface_flush)(surface);
            let data = (api.image_surface_get_data)(surface);
            let stride = (api.image_surface_get_stride)(surface) as usize;
            let mut alpha = Vec::with_capacity(width as usize * height as usize);
            if !data.is_null() {
                for row in 0..height as usize {
                    let line = std::slice::from_raw_parts(data.add(row * stride), width as usize);
                    alpha.extend_from_slice(line);
                }
            }
            (api.surface_destroy)(surface);
            if alpha.is_empty() {
                return None;
            }
            Some(Coverage {
                width: width as u32,
                height: height as u32,
                alpha,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr22_the_title_bar_writes_in_the_desktops_interface_font() {
        assert_eq!(
            family_of_font_name("'Cantarell 11'\n"),
            Some("Cantarell".into())
        );
        assert_eq!(
            family_of_font_name("'Noto Sans CJK KR 10.5'"),
            Some("Noto Sans CJK KR".into())
        );
        assert_eq!(family_of_font_name("'Inter'"), Some("Inter".into()));
        assert_eq!(family_of_font_name("''"), None);
    }
}
