//! `file:` URI 与本地路径互转，以及打开文档的 path key。
//!
//! 不引入 `url` crate：LSP 只需要处理 `file:` URI。百分号按 UTF-8 字节
//! 编码/解码，兼容空格、中文、Windows 盘符与 UNC 路径。

use std::path::{Path, PathBuf};

/// 本地路径 → `file:` URI。
///
/// Windows 盘符路径输出 `file:///C:/...`，UNC 输出 `file://server/share/...`；
/// unix 绝对路径输出 `file:///...`。
pub fn path_to_uri(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    #[cfg(windows)]
    let text = absolute.to_string_lossy().replace('\\', "/");
    #[cfg(not(windows))]
    let text = absolute.to_string_lossy().into_owned();

    #[cfg(windows)]
    {
        if let Some(rest) = text.strip_prefix("//") {
            let (server, share) = rest.split_once('/').unwrap_or((rest, ""));
            let mut uri = format!("file://{}", encode(server));
            if !share.is_empty() {
                uri.push('/');
                uri.push_str(&encode(share));
            }
            return uri;
        }
        let trimmed = text.strip_prefix('/').unwrap_or(&text);
        format!("file:///{}", encode(trimmed))
    }

    #[cfg(not(windows))]
    {
        format!("file://{}", encode(&text))
    }
}

/// `file:` URI → 本地路径字符串；非 `file:` URI 或非法编码返回 `None`。
pub fn uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let (authority, encoded_path) = if let Some(path) = rest.strip_prefix('/') {
        (None, format!("/{path}"))
    } else if let Some(path) = rest.strip_prefix("localhost/") {
        (None, format!("/{path}"))
    } else {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        (Some(authority), format!("/{path}"))
    };
    let decoded = percent_decode(&encoded_path)?;

    #[cfg(windows)]
    {
        if let Some(authority) = authority {
            let share = decoded.trim_start_matches('/').replace('/', "\\");
            return Some(format!("\\\\{authority}\\{share}"));
        }
        let path = decoded.replace('/', "\\");
        let trimmed = path.strip_prefix('\\').unwrap_or(&path);
        if trimmed.as_bytes().get(1) == Some(&b':') {
            return Some(trimmed.to_string());
        }
        Some(path)
    }

    #[cfg(not(windows))]
    {
        if let Some(authority) = authority {
            return Some(format!("//{authority}{decoded}"));
        }
        Some(decoded)
    }
}

/// 打开文档的归一化 key：Windows 下统一分隔符并忽略大小写。
pub(crate) fn path_key(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string()
    }
}

/// 相对路径按 workspace 根解析；绝对路径原样返回。
pub(crate) fn resolve_path(path: &str, workspace_root: &Path) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        workspace_root.join(candidate)
    }
}

fn encode(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                output.push(char::from(byte));
            }
            _ => {
                output.push('%');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 0x0F)]));
            }
        }
    }
    output
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            output.push(high * 16 + low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_drive_uri_round_trips() {
        let path = Path::new(r"C:\Users\a b\项目\main.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///C:/Users/a%20b/%E9%A1%B9%E7%9B%AE/main.rs");
        assert_eq!(
            uri_to_path(&uri).as_deref(),
            Some(r"C:\Users\a b\项目\main.rs")
        );
    }

    #[cfg(windows)]
    #[test]
    fn encoded_drive_colon_and_lowercase_are_accepted() {
        assert_eq!(
            uri_to_path("file:///c%3A/Users/test/main.rs").as_deref(),
            Some(r"c:\Users\test\main.rs")
        );
    }

    #[cfg(windows)]
    #[test]
    fn unc_uri_maps_to_windows_share_path() {
        assert_eq!(
            uri_to_path("file://server/share/dir/main.rs").as_deref(),
            Some(r"\\server\share\dir\main.rs")
        );
    }

    #[cfg(windows)]
    #[test]
    fn localhost_is_treated_as_local_path() {
        assert_eq!(
            uri_to_path("file://localhost/C:/tmp/main.rs").as_deref(),
            Some(r"C:\tmp\main.rs")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_absolute_uri_round_trips() {
        let path = Path::new("/tmp/a b/项目/main.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///tmp/a%20b/%E9%A1%B9%E7%9B%AE/main.rs");
        assert_eq!(uri_to_path(&uri).as_deref(), Some("/tmp/a b/项目/main.rs"));
    }

    #[test]
    fn non_file_or_malformed_uri_is_rejected() {
        assert!(uri_to_path("https://example.com/main.rs").is_none());
        assert!(uri_to_path("file:///C:/tmp/%ZZ.rs").is_none());
        assert!(uri_to_path("file:///C:/tmp/%2.rs").is_none());
    }

    #[test]
    fn relative_paths_resolve_against_workspace() {
        #[cfg(windows)]
        let root = Path::new(r"C:\work");
        #[cfg(not(windows))]
        let root = Path::new("/work");

        assert_eq!(resolve_path("src/main.rs", root), root.join("src/main.rs"));

        #[cfg(windows)]
        let absolute = Path::new(r"C:\elsewhere\main.rs");
        #[cfg(not(windows))]
        let absolute = Path::new("/elsewhere/main.rs");
        assert_eq!(resolve_path(&absolute.to_string_lossy(), root), absolute);
    }

    #[test]
    fn path_key_normalizes_separators_on_windows() {
        #[cfg(windows)]
        assert_eq!(path_key(r"C:\Work\Main.RS"), path_key("c:/work/main.rs"));
        #[cfg(not(windows))]
        assert_eq!(path_key("/Work/Main.RS"), "/Work/Main.RS");
    }
}
