/// First statement of every nonempty generated stylesheet.
pub const LAYER_ORDER: &str = "@layer zw-reset, zw-tokens, zfb-hi, base, components;\n";

pub(crate) fn wrap(header: &str, contents: &str) -> String {
    let mut output = format!("{header} {{\n");
    for line in contents.lines() {
        output.push_str("  ");
        output.push_str(line);
        output.push('\n');
    }
    output.push_str("}\n");
    output
}
