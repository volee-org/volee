//! The web interface's bundle, embedded in the binary at build time
//! (`build.rs`), and the handler that serves it.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

/// Whether the real interface is embedded, not the stub page.
pub(super) fn is_bundled() -> bool {
    BUNDLED
}

/// The bytes of the bundle's file at `path` (no leading slash).
fn find(path: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, bytes)| *bytes)
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Serves `GET /` and the files of the bundle. The bundle is public: the
/// token protects the data, not the page's code, and the page needs to load
/// before it can read the token from its address.
pub(crate) async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(bytes) = find(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // Built files carry a hash in their name and never change; the page that
    // names them must always be read again.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_has_a_page_to_serve() {
        let page = find("index.html").expect("an index page, real or stub");
        assert!(std::str::from_utf8(page).unwrap().contains("<html"));
    }

    #[test]
    fn files_are_told_apart_by_their_extension() {
        assert_eq!(
            content_type("assets/index-ab12.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type("assets/index-ab12.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("data"), "application/octet-stream");
    }

    #[test]
    fn an_unknown_file_is_not_found_and_a_path_cannot_climb() {
        assert!(find("nope.js").is_none());
        assert!(find("../Cargo.toml").is_none());
    }
}
