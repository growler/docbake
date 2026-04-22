pub(super) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

pub(super) fn xml_attr_escape(s: &str) -> String {
    xml_escape(s)
}

/// Escape text for CDATA sections. CDATA cannot contain `]]>`, so we
/// split the CDATA section around it.
pub(super) fn cdata_escape(s: &str) -> String {
    s.replace("]]>", "]]]]><![CDATA[>")
}
