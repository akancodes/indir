//! Picking a file name for a download: the server's suggestion, else the
//! link's last segment, made safe for Windows, macOS and Linux alike.

use percent_encoding::percent_decode;
use url::Url;

/// The longest name kept, in bytes. File systems allow 255; the rest leaves
/// room for " (12)" and the partial file's extension.
const MAX_NAME: usize = 200;

/// The name a download is saved under, from what the server said about it.
pub(crate) fn suggested(disposition: Option<&[u8]>, url: &Url, content_type: Option<&str>) -> String {
    let named = disposition.and_then(from_disposition).map(|name| sanitize(&name)).filter(|name| !name.is_empty());
    let mut name = named.unwrap_or_else(|| url_name(url));
    if split_extension(&name).1.is_empty() {
        if let Some(extension) = content_type.and_then(extension_for) {
            name = format!("{name}.{extension}");
        }
    }
    name
}

/// A name for a link before the server has been asked: its last path
/// segment, or "download".
pub fn link_name(link: &str) -> String {
    Url::parse(link).map(|url| url_name(&url)).unwrap_or_else(|_| "download".into())
}

fn url_name(url: &Url) -> String {
    let segment = url.path_segments().and_then(|mut segments| segments.rfind(|s| !s.is_empty())).unwrap_or_default();
    let name = sanitize(&percent_decode(segment.as_bytes()).decode_utf8_lossy());
    if name.is_empty() {
        "download".into()
    } else {
        name
    }
}

/// Makes a name safe everywhere: no folders, none of the characters Windows
/// forbids, no trailing dots or spaces, no device names like `CON`, and not
/// too long.
pub fn sanitize(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = name
        .chars()
        .filter(|&c| !is_invisible(c))
        .map(|c| if c.is_control() || r#"<>:"|?*"#.contains(c) { '_' } else { c })
        .collect();
    let mut name = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if is_device_name(&name) {
        name.insert(0, '_');
    }
    shorten(name, MAX_NAME)
}

/// Characters that change how a name looks without showing themselves: the
/// bidi controls would let "invoice\u{202E}fdp.exe" pass for "invoiceexe.pdf".
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// "report.pdf" → "report (2).pdf", "backup.tar.gz" → "backup (2).tar.gz".
pub(crate) fn numbered(name: &str, n: u32) -> String {
    let (stem, extension) = split_extension(name);
    format!("{stem} ({n}){extension}")
}

/// Windows reserves these names in every folder, with any extension.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9')
        }
    }
}

/// Cuts a name to `max` bytes, keeping its extension and whole characters.
fn shorten(name: String, max: usize) -> String {
    if name.len() <= max {
        return name;
    }
    let (stem, extension) = split_extension(&name);
    let mut end = max.saturating_sub(extension.len()).min(stem.len());
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &stem[..end], extension)
}

/// "report.pdf" → ("report", ".pdf"), "backup.tar.gz" → ("backup", ".tar.gz");
/// ".bashrc" and "README" have no extension.
fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for double in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if lower.len() > double.len() && lower.ends_with(double) {
            let at = name.len() - double.len();
            return (&name[..at], &name[at..]);
        }
    }
    match name.rfind('.') {
        Some(at) if at > 0 && at + 1 < name.len() => (&name[..at], &name[at..]),
        _ => (name, ""),
    }
}

/// The file name in a Content-Disposition header (RFC 6266). The extended
/// `filename*` (RFC 5987: any charset, percent-encoded) wins over `filename`.
fn from_disposition(header: &[u8]) -> Option<String> {
    let header = String::from_utf8_lossy(header);
    let mut plain = None;
    let mut extended = None;
    for (key, value) in parameters(&header) {
        if key.eq_ignore_ascii_case("filename*") {
            extended = extended.or_else(|| decode_extended(&value));
        } else if key.eq_ignore_ascii_case("filename") {
            plain = plain.or(Some(value));
        }
    }
    extended.or(plain).filter(|name| !name.trim().is_empty())
}

/// The `key=value` pairs after the disposition type, quoted values unescaped.
fn parameters(header: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let Some(at) = header.find(';') else { return pairs };
    let mut rest = &header[at + 1..];
    loop {
        rest = rest.trim_start_matches([' ', '\t', ';']);
        let Some(equals) = rest.find('=') else { break };
        // A parameter without a value ("attachment; foo; filename=…"): skip it.
        if let Some(semicolon) = rest[..equals].find(';') {
            rest = &rest[semicolon + 1..];
            continue;
        }
        let key = rest[..equals].trim().to_string();
        rest = rest[equals + 1..].trim_start();
        let value = if let Some(quoted) = rest.strip_prefix('"') {
            let mut value = String::new();
            let mut end = quoted.len();
            let mut chars = quoted.char_indices();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next().map(|(_, escaped)| escaped)),
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => value.push(c),
                }
            }
            rest = &quoted[end..];
            value
        } else {
            let end = rest.find(';').unwrap_or(rest.len());
            let value = rest[..end].trim().to_string();
            rest = &rest[end..];
            value
        };
        pairs.push((key, value));
    }
    pairs
}

/// `UTF-8''r%C3%A9sum%C3%A9.pdf` → "résumé.pdf".
fn decode_extended(value: &str) -> Option<String> {
    let mut fields = value.splitn(3, '\'');
    let charset = fields.next()?.trim();
    let _language = fields.next()?;
    let bytes: Vec<u8> = percent_decode(fields.next()?.as_bytes()).collect();
    if charset.eq_ignore_ascii_case("utf-8") {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.iter().map(|&b| char::from(b)).collect())
    } else {
        None
    }
}

/// An extension for a name that has none, from the server's Content-Type.
fn extension_for(content_type: &str) -> Option<&'static str> {
    let essence = content_type.split(';').next()?.trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "text/html" => "html",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "application/json" => "json",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/x-tar" => "tar",
        "application/vnd.android.package-archive" => "apk",
        "application/x-msdownload" | "application/vnd.microsoft.portable-executable" => "exe",
        "application/x-apple-diskimage" => "dmg",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(disposition: &str) -> Option<String> {
        from_disposition(disposition.as_bytes())
    }

    #[test]
    fn reads_content_disposition() {
        assert_eq!(named("attachment; filename=report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(named(r#"attachment; filename="my report.pdf""#).as_deref(), Some("my report.pdf"));
        assert_eq!(named(r#"attachment; filename="say \"hi\".txt""#).as_deref(), Some(r#"say "hi".txt"#));
        assert_eq!(named("attachment;filename=a.txt;size=12").as_deref(), Some("a.txt"));
        assert_eq!(named("attachment; foo; filename=b.txt").as_deref(), Some("b.txt"));
        assert_eq!(named("inline").as_deref(), None);
        assert_eq!(named(r#"attachment; filename="""#).as_deref(), None);
    }

    #[test]
    fn prefers_the_extended_name() {
        let header = r#"attachment; filename="fallback.pdf"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"#;
        assert_eq!(named(header).as_deref(), Some("résumé.pdf"));
        assert_eq!(named("attachment; filename*=iso-8859-1'en'%A3%20rates.txt").as_deref(), Some("£ rates.txt"));
        assert_eq!(named("attachment; filename*=UTF-8''%C3%B6zet.txt; filename=ozet.txt").as_deref(), Some("özet.txt"));
        // A charset nobody uses falls back to the plain name.
        assert_eq!(named("attachment; filename*=koi8-r''%C1.txt; filename=plain.txt").as_deref(), Some("plain.txt"));
    }

    #[test]
    fn reads_raw_utf8_names() {
        assert_eq!(from_disposition("attachment; filename=\"çiçek.jpg\"".as_bytes()).as_deref(), Some("çiçek.jpg"));
    }

    #[test]
    fn names_from_links() {
        assert_eq!(link_name("https://example.com/files/ubuntu.iso"), "ubuntu.iso");
        assert_eq!(link_name("https://example.com/files/my%20file.zip?token=1"), "my file.zip");
        assert_eq!(link_name("https://example.com/a/b/"), "b");
        assert_eq!(link_name("https://example.com/"), "download");
        assert_eq!(link_name("not a link"), "download");
    }

    #[test]
    fn suggests_an_extension_from_the_content_type() {
        let url = Url::parse("https://example.com/download?id=4").unwrap();
        assert_eq!(suggested(None, &url, Some("application/zip")), "download.zip");
        assert_eq!(suggested(None, &url, Some("text/html; charset=utf-8")), "download.html");
        assert_eq!(suggested(None, &url, Some("application/octet-stream")), "download");
        let url = Url::parse("https://example.com/song.mp3").unwrap();
        assert_eq!(suggested(None, &url, Some("audio/mpeg")), "song.mp3");
        assert_eq!(suggested(Some(b"attachment; filename=../../etc/passwd"), &url, None), "passwd");
    }

    #[test]
    fn makes_names_safe() {
        assert_eq!(sanitize("a<b>c:d\"e|f?g*h.txt"), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(sanitize("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(sanitize("  spaced out . "), "spaced out");
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("com1"), "_com1");
        assert_eq!(sanitize("COM0.txt"), "COM0.txt");
        assert_eq!(sanitize("console.txt"), "console.txt");
        assert_eq!(sanitize("tab\there"), "tab_here");
        assert_eq!(sanitize(".."), "");
    }

    #[test]
    fn drops_characters_that_disguise_a_name() {
        assert_eq!(sanitize("invoice\u{202E}fdp.exe"), "invoicefdp.exe");
        assert_eq!(sanitize("\u{2067}photo\u{2069}.jpg\u{200F}"), "photo.jpg");
        assert_eq!(sanitize("\u{FEFF}notes.txt"), "notes.txt");
    }

    #[test]
    fn shortens_long_names_but_keeps_the_extension() {
        let long = format!("{}.mp4", "ş".repeat(150));
        let short = sanitize(&long);
        assert!(short.len() <= MAX_NAME, "{}", short.len());
        assert!(short.ends_with("ş.mp4"));
    }

    #[test]
    fn numbers_names() {
        assert_eq!(numbered("report.pdf", 2), "report (2).pdf");
        assert_eq!(numbered("backup.TAR.GZ", 1), "backup (1).TAR.GZ");
        assert_eq!(numbered("README", 3), "README (3)");
        assert_eq!(numbered(".bashrc", 1), ".bashrc (1)");
    }
}
