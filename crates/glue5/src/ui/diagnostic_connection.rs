//! Clipboard export only. Never install configuration or log its credentials.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Codex,
    GenericJson,
}

// JSON and TOML basic strings share these escapes. Keep literal Unicode;
// generated LAN addresses and pairing tokens are ASCII, but still quote safely.
fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn config(format: Format, url: &str, token: &str) -> String {
    let url = quoted(url);
    let authorization = quoted(&format!("Bearer {token}"));
    match format {
        Format::Codex => format!(
            "[mcp_servers.glue5]\nurl = {url}\nenabled_tools = [\"diagnostic_control\", \"logs\"]\ndefault_tools_approval_mode = \"prompt\"\nhttp_headers = {{ Authorization = {authorization} }}\n"
        ),
        Format::GenericJson => format!(
            "{{\n  \"mcpServers\": {{\n    \"glue5\": {{\n      \"url\": {url},\n      \"headers\": {{ \"Authorization\": {authorization} }}\n    }}\n  }}\n}}\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_export_uses_toml_headers_and_limits_tools() {
        let text = config(Format::Codex, "http://192.0.2.1:47110/mcp", "EXAMPLE-NOT-A-TOKEN");
        assert!(text.starts_with("[mcp_servers.glue5]\n"));
        assert!(text.contains("http_headers = { Authorization = \"Bearer EXAMPLE-NOT-A-TOKEN\" }"));
        assert!(text.contains("enabled_tools = [\"diagnostic_control\", \"logs\"]"));
        assert!(text.contains("default_tools_approval_mode = \"prompt\""));
        assert!(!text.contains("mcpServers"));
    }

    #[test]
    fn generic_export_and_quoting_are_explicit() {
        let text = config(Format::GenericJson, "http://192.0.2.1:47110/mcp", "EXAMPLE");
        assert!(text.contains("\"mcpServers\""));
        assert!(text.contains("\"headers\": { \"Authorization\": \"Bearer EXAMPLE\" }"));
        assert_eq!(quoted("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(quoted("\t\u{0001}"), "\"\\t\\u0001\"");
    }
}
