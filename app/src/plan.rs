//! Rasterizes SVGs (plan previews, the logo) for display.

use anyhow::{anyhow, Result};
use gpui::RenderImage;
use resvg::{tiny_skia, usvg};
use std::sync::{Arc, OnceLock};

/// System fonts, loaded once: the previews contain labels.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

fn pixmap(svg: &str, scale: f32) -> Result<tiny_skia::Pixmap> {
    let options = usvg::Options { fontdb: fonts(), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &options)?;
    let size = tree.size();
    let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("empty SVG"))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Ok(pixmap)
}

/// PNG bytes of `svg`, rendered at `scale` times its own size.
pub fn rasterize(svg: &str, scale: f32) -> Result<Vec<u8>> {
    Ok(pixmap(svg, scale)?.encode_png()?)
}

/// `svg` rendered at `scale` times its own size, ready for `Window::paint_image`.
pub fn render(svg: &str, scale: f32) -> Result<Arc<RenderImage>> {
    let pixmap = pixmap(svg, scale)?;
    let (w, h) = (pixmap.width(), pixmap.height());
    // GPUI expects BGRA; the previews are opaque, so premultiplied alpha needs no care.
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(w, h, data).ok_or_else(|| anyhow!("preview buffer size"))?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}
