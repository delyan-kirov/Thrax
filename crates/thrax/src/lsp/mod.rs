//! `thrax lsp`: the compiler as a language server, over JSON-RPC on stdio.
//!
//! The server is the compiler (as in Flix): every diagnostic, fix and type it
//! shows is the one `thrax check` would produce. See `documentation/lsp.md`
//! for the design, in particular how diagnostics are ranked so the most
//! important error stands out.

pub mod analysis;
pub mod clock;
pub mod server;
pub mod text;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use utilities::Json;

/// Read one `Content-Length`-framed message. `Ok(None)` at end of input.
pub fn read_message(input: &mut impl BufRead) -> std::io::Result<Option<Json>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            length = v.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length.expect("checked above")];
    input.read_exact(&mut body)?;
    let text = String::from_utf8_lossy(&body);
    Ok(Some(Json::parse(&text).unwrap_or(Json::Null)))
}

pub fn write_message(out: &mut impl Write, msg: &Json) -> std::io::Result<()> {
    let body = msg.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    out.flush()
}

/// The file a `file://` URI names (percent-escapes decoded), canonicalized so
/// it matches the paths the loader resolves. A file that does not exist yet
/// (an unsaved buffer) has its directory canonicalized instead: on Windows
/// that adds the same `\\?\` prefix the loader's sibling lookup carries.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                bytes.push(b);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let text = String::from_utf8(bytes).ok()?;
    let path = PathBuf::from(if cfg!(windows) { from_uri_path(&text) } else { text });
    if let Ok(real) = path.canonicalize() {
        return Some(real);
    }
    let dir = path.parent().zip(path.file_name()).and_then(|(dir, name)| Some(dir.canonicalize().ok()?.join(name)));
    Some(dir.unwrap_or(path))
}

/// `/C:/x/y` (a URI path) as the Windows path `C:\x\y`; `//server/share/x`
/// stays a UNC path `\\server\share\x`.
fn from_uri_path(text: &str) -> String {
    let b = text.as_bytes();
    let text = if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' { &text[1..] } else { text };
    text.replace('/', "\\")
}

/// A Windows path as a URI path: no `\\?\` verbatim prefix (which
/// `canonicalize` adds), forward slashes, and a leading `/` before the drive.
fn to_uri_path(text: &str) -> String {
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(text).to_string(),
    };
    let text = text.replace('\\', "/");
    let b = text.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' { format!("/{text}") } else { text }
}

pub fn path_to_uri(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    let text = if cfg!(windows) { to_uri_path(&text) } else { text };
    let mut out = String::from("file://");
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Run the server on stdin/stdout until the client says `exit`.
pub fn cmd_lsp() -> std::process::ExitCode {
    server::run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_through_escapes() {
        let p = std::path::Path::new("/tmp/a b/c.thx");
        let uri = path_to_uri(p);
        assert_eq!(uri, "file:///tmp/a%20b/c.thx");
        assert_eq!(uri_to_path(&uri).unwrap(), p);
    }

    #[test]
    fn windows_paths_become_drive_uris() {
        assert_eq!(to_uri_path(r"\\?\D:\a\b c.thx"), "/D:/a/b c.thx");
        assert_eq!(to_uri_path(r"C:\x"), "/C:/x");
        assert_eq!(to_uri_path(r"\\?\UNC\srv\share\x"), "//srv/share/x");
        assert_eq!(from_uri_path("/D:/a/b.thx"), r"D:\a\b.thx");
        assert_eq!(from_uri_path("//srv/share/x"), r"\\srv\share\x");
    }

    #[test]
    fn frames_round_trip() {
        let msg = Json::obj([("jsonrpc", Json::str("2.0")), ("id", Json::int(1))]);
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let back = read_message(&mut std::io::Cursor::new(buf)).unwrap().unwrap();
        assert_eq!(back, msg);
    }
}
