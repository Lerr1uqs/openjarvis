//! Standalone helper that converts Playwright AI snapshot text into a stable YAML AST.

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use serde::Serialize;
use serde_yaml::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::PathBuf,
};
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;

/// Command-line options for the AI snapshot parsing helper.
#[derive(Debug, Clone, Parser)]
#[command(name = "browser_ai_snapshot_parse")]
struct BrowserAiSnapshotParseCli {
    /// Optional input file that contains raw AI snapshot text. Reads stdin when omitted.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Optional output path used to persist the normalized YAML result. Prints to stdout when omitted.
    #[arg(long)]
    output: Option<PathBuf>,
}

/// Stable YAML document emitted by the AI snapshot parser.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct ParsedAiSnapshotDocument {
    format: String,
    version: u32,
    summary: SnapshotConceptSummary,
    nodes: Vec<SnapshotNode>,
}

/// Concept summary extracted from one AI snapshot.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct SnapshotConceptSummary {
    roles: Vec<String>,
    attributes: Vec<String>,
    properties: Vec<String>,
}

/// One parsed AI snapshot node.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SnapshotNode {
    Role {
        role: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        attributes: BTreeMap<String, Value>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        props: BTreeMap<String, Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        children: Vec<SnapshotNode>,
    },
    Text {
        text: String,
    },
}

/// Temporary representation of one role descriptor parsed from the AI snapshot key text.
#[derive(Debug, Clone, PartialEq)]
struct ParsedRoleDescriptor {
    role: String,
    name: Option<String>,
    reference: Option<String>,
    attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Default)]
struct ConceptCollector {
    roles: BTreeSet<String>,
    attributes: BTreeSet<String>,
    properties: BTreeSet<String>,
}

impl ConceptCollector {
    fn record_role(&mut self, role: &str) {
        self.roles.insert(role.to_string());
    }

    fn record_attribute(&mut self, attribute: &str) {
        self.attributes.insert(attribute.to_string());
    }

    fn record_property(&mut self, property: &str) {
        self.properties.insert(property.to_string());
    }

    fn finish(self) -> SnapshotConceptSummary {
        SnapshotConceptSummary {
            roles: self.roles.into_iter().collect(),
            attributes: self.attributes.into_iter().collect(),
            properties: self.properties.into_iter().collect(),
        }
    }
}

/// Parse one AI snapshot payload and return a stable YAML AST document.
///
/// # 示例
/// ```rust,ignore
/// let snapshot = r#"
/// - button "Search" [ref=e4]: Go
/// "#;
/// let parsed = parse_ai_snapshot_text(snapshot)?;
/// assert_eq!(parsed.format, "playwright-ai-snapshot");
/// ```
fn parse_ai_snapshot_text(snapshot: &str) -> Result<ParsedAiSnapshotDocument> {
    let parsed: Value =
        serde_yaml::from_str(snapshot).context("failed to parse AI snapshot YAML text")?;
    let items = parsed
        .as_sequence()
        .ok_or_else(|| anyhow!("AI snapshot top-level must be a YAML sequence"))?;

    let mut concepts = ConceptCollector::default();
    let nodes = parse_sequence(items, &mut concepts)?;

    Ok(ParsedAiSnapshotDocument {
        format: "playwright-ai-snapshot".to_string(),
        version: 1,
        summary: concepts.finish(),
        nodes,
    })
}

/// Execute the AI snapshot parsing helper.
///
/// # 示例
/// ```rust,no_run
/// let cli = clap::Parser::parse_from([
///     "browser_ai_snapshot_parse",
///     "--input",
///     "snapshot.yaml",
/// ]);
/// let _ = cli;
/// ```
fn run_browser_ai_snapshot_parse(cli: &BrowserAiSnapshotParseCli) -> Result<()> {
    let raw_snapshot = read_input(cli)?;
    info!(
        input = cli
            .input
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "stdin".to_string()),
        "parsing browser AI snapshot"
    );
    let parsed = parse_ai_snapshot_text(&raw_snapshot)?;
    debug!(
        node_count = parsed.nodes.len(),
        role_count = parsed.summary.roles.len(),
        attribute_count = parsed.summary.attributes.len(),
        property_count = parsed.summary.properties.len(),
        "browser AI snapshot parsed"
    );

    let output = serde_yaml::to_string(&parsed).context("failed to render parsed YAML output")?;
    write_output(cli, &output)?;
    Ok(())
}

/// Parse one YAML sequence into normalized snapshot nodes.
fn parse_sequence(items: &[Value], concepts: &mut ConceptCollector) -> Result<Vec<SnapshotNode>> {
    let mut nodes = Vec::new();
    for item in items {
        nodes.extend(parse_sequence_item(item, concepts)?);
    }
    Ok(nodes)
}

/// Parse one YAML sequence item into one or more normalized nodes.
fn parse_sequence_item(item: &Value, concepts: &mut ConceptCollector) -> Result<Vec<SnapshotNode>> {
    match item {
        Value::String(raw) => Ok(vec![parse_role_node(raw, None, concepts)?]),
        Value::Mapping(map) => parse_mapping_item(map, concepts),
        other => bail!(
            "AI snapshot items must be strings or mappings, found `{}`",
            value_type_name(other)
        ),
    }
}

/// Parse one mapping item inside the YAML sequence.
fn parse_mapping_item(
    map: &serde_yaml::Mapping,
    concepts: &mut ConceptCollector,
) -> Result<Vec<SnapshotNode>> {
    let mut nodes = Vec::new();
    for (key, value) in map {
        let key = key
            .as_str()
            .ok_or_else(|| anyhow!("AI snapshot mapping keys must be strings"))?;
        if key == "text" {
            nodes.push(SnapshotNode::Text {
                text: scalar_to_string(value)?,
            });
            continue;
        }
        if key.starts_with('/') {
            bail!("property node `{key}` can only appear as a child of one role node");
        }
        nodes.push(parse_role_node(key, Some(value), concepts)?);
    }
    Ok(nodes)
}

/// Parse one role node and its optional payload.
fn parse_role_node(
    key: &str,
    value: Option<&Value>,
    concepts: &mut ConceptCollector,
) -> Result<SnapshotNode> {
    let descriptor = parse_role_descriptor(key)?;
    concepts.record_role(&descriptor.role);
    for attribute in descriptor.attributes.keys() {
        concepts.record_attribute(attribute);
    }

    let mut props = BTreeMap::new();
    let mut children = Vec::new();
    let mut scalar_value = None;

    if let Some(value) = value {
        match value {
            Value::Sequence(items) => {
                for item in items {
                    if let Some((property_name, property_value)) = try_parse_property(item)? {
                        concepts.record_property(&property_name);
                        props.insert(property_name, property_value);
                        continue;
                    }
                    children.extend(parse_sequence_item(item, concepts)?);
                }
            }
            Value::Null => {}
            other if is_scalar_value(other) => {
                scalar_value = Some(other.clone());
            }
            other => bail!(
                "role node `{key}` must map to a scalar, sequence, or null, found `{}`",
                value_type_name(other)
            ),
        }
    }

    Ok(SnapshotNode::Role {
        role: descriptor.role,
        name: descriptor.name,
        reference: descriptor.reference,
        attributes: descriptor.attributes,
        props,
        value: scalar_value,
        children,
    })
}

/// Parse one role descriptor from the AI snapshot line-oriented key.
fn parse_role_descriptor(input: &str) -> Result<ParsedRoleDescriptor> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("AI snapshot role descriptor must not be empty");
    }

    let bytes = trimmed.as_bytes();
    let mut position = 0;
    while position < bytes.len()
        && (bytes[position].is_ascii_alphanumeric()
            || bytes[position] == b'_'
            || bytes[position] == b'-')
    {
        position += 1;
    }
    if position == 0 {
        bail!("failed to parse role name from `{trimmed}`");
    }

    let role = trimmed[..position].to_string();
    let mut name = None;
    let mut attributes = BTreeMap::new();

    while position < bytes.len() {
        skip_ascii_whitespace(bytes, &mut position);
        if position >= bytes.len() {
            break;
        }
        match bytes[position] {
            b'"' => {
                let (parsed_name, next_position) = parse_quoted_string(trimmed, position)?;
                if name.is_some() {
                    bail!("role descriptor `{trimmed}` contains multiple names");
                }
                name = Some(parsed_name);
                position = next_position;
            }
            b'[' => {
                let (attribute, attribute_value, next_position) =
                    parse_attribute_segment(trimmed, position)?;
                attributes.insert(attribute, attribute_value);
                position = next_position;
            }
            _ => bail!("unexpected token in role descriptor `{trimmed}`"),
        }
    }

    let reference = attributes
        .get("ref")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(ParsedRoleDescriptor {
        role,
        name,
        reference,
        attributes,
    })
}

/// Try to parse one property item like `/url: /docs`.
fn try_parse_property(item: &Value) -> Result<Option<(String, Value)>> {
    let Value::Mapping(map) = item else {
        return Ok(None);
    };
    if map.len() != 1 {
        return Ok(None);
    }

    let mut entries = map.iter();
    let Some((key, value)) = entries.next() else {
        return Ok(None);
    };
    let Some(key) = key.as_str() else {
        return Ok(None);
    };
    if !key.starts_with('/') {
        return Ok(None);
    }

    if !is_scalar_value(value) && !matches!(value, Value::Null) {
        bail!("property node `{key}` must map to a scalar or null value");
    }

    Ok(Some((
        key.trim_start_matches('/').to_string(),
        value.clone(),
    )))
}

/// Parse one `[attribute]` or `[attribute=value]` segment.
fn parse_attribute_segment(input: &str, start: usize) -> Result<(String, Value, usize)> {
    let bytes = input.as_bytes();
    let mut position = start;
    if bytes.get(position) != Some(&b'[') {
        bail!("attribute segment must start with `[`");
    }
    position += 1;
    let content_start = position;
    while position < bytes.len() && bytes[position] != b']' {
        position += 1;
    }
    if position >= bytes.len() {
        bail!("unterminated attribute segment in `{input}`");
    }
    let content = input[content_start..position].trim();
    if content.is_empty() {
        bail!("empty attribute segment in `{input}`");
    }
    position += 1;

    let mut parts = content.splitn(2, '=');
    let key = parts
        .next()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .ok_or_else(|| anyhow!("invalid empty attribute name in `{input}`"))?;
    let value = match parts.next() {
        Some(raw) => parse_scalar_literal(raw.trim()),
        None => Value::Bool(true),
    };

    Ok((key.to_string(), value, position))
}

/// Parse one quoted accessible name.
fn parse_quoted_string(input: &str, start: usize) -> Result<(String, usize)> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        bail!("quoted string must start with `\"`");
    }

    let mut output = String::new();
    let mut escaped = false;
    for (offset, ch) in input[start + 1..].char_indices() {
        if escaped {
            output.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                escaped = true;
            }
            '"' => {
                return Ok((output, start + 1 + offset + ch.len_utf8()));
            }
            _ => {
                output.push(ch);
            }
        }
    }

    bail!("unterminated quoted name in `{input}`")
}

/// Parse one scalar literal found in an AI snapshot attribute.
fn parse_scalar_literal(raw: &str) -> Value {
    if raw.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(number) = raw.parse::<i64>() {
        return Value::Number(number.into());
    }
    Value::String(raw.to_string())
}

/// Convert one scalar YAML value into a printable string.
fn scalar_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Bool(boolean) => Ok(boolean.to_string()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Null => Ok(String::new()),
        other => bail!(
            "expected one scalar YAML value, found `{}` instead",
            value_type_name(other)
        ),
    }
}

/// Read raw snapshot input from the requested file or stdin.
fn read_input(cli: &BrowserAiSnapshotParseCli) -> Result<String> {
    if let Some(path) = &cli.input {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read AI snapshot input {}", path.display()));
    }

    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .context("failed to read AI snapshot input from stdin")?;
    if buffer.trim().is_empty() {
        bail!("AI snapshot input is empty");
    }
    Ok(buffer)
}

/// Persist the rendered YAML output to the requested destination.
fn write_output(cli: &BrowserAiSnapshotParseCli, output: &str) -> Result<()> {
    if let Some(path) = &cli.output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create AI snapshot output dir {}",
                    parent.display()
                )
            })?;
        }
        fs::write(path, output)
            .with_context(|| format!("failed to write AI snapshot output {}", path.display()))?;
        info!(output = %path.display(), "wrote parsed browser AI snapshot");
        return Ok(());
    }

    print!("{output}");
    Ok(())
}

/// Initialize script-local tracing for debug visibility.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off")),
        )
        .without_time()
        .try_init();
}

fn skip_ascii_whitespace(bytes: &[u8], position: &mut usize) {
    while *position < bytes.len() && bytes[*position].is_ascii_whitespace() {
        *position += 1;
    }
}

fn is_scalar_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged",
    }
}

/// Main entrypoint for the AI snapshot parsing helper.
fn main() -> Result<()> {
    init_tracing();
    let cli = BrowserAiSnapshotParseCli::parse();
    run_browser_ai_snapshot_parse(&cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SNAPSHOT: &str = r#"
- main [ref=e2]:
  - heading "Title" [level=1] [ref=e3]
  - text: Plain text before
  - link "Docs" [ref=e4] [cursor=pointer]:
    - /url: /docs
  - button "Save" [ref=e5]
  - button "TriState" [pressed=mixed] [ref=e6]
  - textbox "Search docs" [active] [ref=e7]
  - checkbox "Remember me" [checked] [ref=e8]
  - combobox "Country" [ref=e10]:
    - option "CN"
    - option "US" [selected]
  - iframe [ref=e20]:
    - generic [active] [ref=f1e1]:
      - heading "Inner" [level=2] [ref=f1e2]
      - button "Open menu" [expanded] [ref=f1e3]
"#;

    #[test]
    fn parse_ai_snapshot_preserves_tree_and_concepts() -> Result<()> {
        // 测试场景: 复杂 AI snapshot 需要保留层级、属性和概念汇总，供后续规则处理。
        let document = parse_ai_snapshot_text(SAMPLE_SNAPSHOT)?;

        assert_eq!(document.format, "playwright-ai-snapshot");
        assert!(document.summary.roles.contains(&"main".to_string()));
        assert!(document.summary.roles.contains(&"iframe".to_string()));
        assert!(document.summary.attributes.contains(&"ref".to_string()));
        assert!(document.summary.attributes.contains(&"cursor".to_string()));
        assert!(
            document
                .summary
                .attributes
                .contains(&"expanded".to_string())
        );
        assert_eq!(document.summary.properties, vec!["url".to_string()]);
        assert_eq!(document.nodes.len(), 1);

        let SnapshotNode::Role {
            role,
            reference,
            children,
            ..
        } = &document.nodes[0]
        else {
            bail!("root node should be one role node");
        };
        assert_eq!(role, "main");
        assert_eq!(reference.as_deref(), Some("e2"));
        assert!(children.iter().any(|child| matches!(
            child,
            SnapshotNode::Text { text } if text == "Plain text before"
        )));

        let link_node = children.iter().find(|child| matches!(
            child,
            SnapshotNode::Role { role, name, .. } if role == "link" && name.as_deref() == Some("Docs")
        ));
        let Some(SnapshotNode::Role { props, .. }) = link_node else {
            bail!("expected to find parsed link node");
        };
        assert_eq!(props.get("url"), Some(&Value::String("/docs".to_string())));

        Ok(())
    }

    #[test]
    fn parse_role_descriptor_supports_names_and_attributes() -> Result<()> {
        // 测试场景: descriptor 里的 role、name、布尔属性与标量属性都要被稳定拆解。
        let parsed = parse_role_descriptor(r#"button "Search" [pressed=mixed] [ref=e4]"#)?;

        assert_eq!(parsed.role, "button");
        assert_eq!(parsed.name.as_deref(), Some("Search"));
        assert_eq!(parsed.reference.as_deref(), Some("e4"));
        assert_eq!(
            parsed.attributes.get("pressed"),
            Some(&Value::String("mixed".to_string()))
        );
        assert_eq!(
            parsed.attributes.get("ref"),
            Some(&Value::String("e4".to_string()))
        );

        Ok(())
    }

    #[test]
    fn parse_role_descriptor_preserves_utf8_names() -> Result<()> {
        // 测试场景: AI snapshot 中的中文名称不能被按字节拆坏，必须完整保留 UTF-8 文本。
        let parsed = parse_role_descriptor(r#"textbox "百度一下，你就知道" [active]"#)?;

        assert_eq!(parsed.role, "textbox");
        assert_eq!(parsed.name.as_deref(), Some("百度一下，你就知道"));
        assert_eq!(parsed.attributes.get("active"), Some(&Value::Bool(true)));

        Ok(())
    }

    #[test]
    fn parse_ai_snapshot_rejects_invalid_top_level_property() {
        // 测试场景: 顶层 property 不是合法 AI snapshot 节点，解析器需要显式失败。
        let error =
            parse_ai_snapshot_text("- /url: /bad").expect_err("invalid root property should fail");
        assert!(error.to_string().contains("property node"));
    }

    #[test]
    fn parse_ai_snapshot_rejects_non_sequence_root() {
        // 测试场景: 非 sequence 根节点不是合法 snapshot 文本，解析器需要返回根级错误。
        let error = parse_ai_snapshot_text("button: Save").expect_err("mapping root should fail");
        assert!(
            error
                .to_string()
                .contains("top-level must be a YAML sequence")
        );
    }
}
