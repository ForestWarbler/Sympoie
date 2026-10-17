//! The Sympoie logo: the "Nocturne" app icon, a gold clef S on midnight plum.
//!
//! The vector masters live in `assets/branding/`. The window icon and the About
//! dialog use a pre-rendered 256 px RGBA copy so no image decoder is needed.

use eframe::egui;

/// Side length, in pixels, of the embedded icon bitmap.
pub const ICON_SIZE: u32 = 256;

/// Unpremultiplied RGBA pixels of `assets/branding/sympoie-icon-256.png`.
pub const ICON_RGBA: &[u8] = include_bytes!("../../assets/branding/sympoie-icon-256.rgba");

/// The app icon for the window, dock and taskbar.
pub fn icon_data() -> egui::IconData {
    egui::IconData {
        rgba: ICON_RGBA.to_vec(),
        width: ICON_SIZE,
        height: ICON_SIZE,
    }
}

/// The logo as a texture, uploaded once and then reused.
pub fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("sympoie-logo");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let side = ICON_SIZE as usize;
    let image = egui::ColorImage::from_rgba_unmultiplied([side, side], ICON_RGBA);
    let texture = ctx.load_texture("sympoie-logo", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_matches_its_size() {
        assert_eq!(ICON_RGBA.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }
}
