use include_dir::{Dir, include_dir};

static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets");

pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {
    let file = ASSETS.get_file(path)?;
    Some((file.contents(), mime_for(path)))
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}
