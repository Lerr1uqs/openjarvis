use openjarvis::agent::{ReadTool, ToolHandler, ToolSchemaProtocol};

#[test]
fn generated_tool_schema_can_be_projected_for_multiple_protocols() {
    let definition = ReadTool::new().definition();

    let openai_schema = definition
        .input_schema
        .for_protocol(ToolSchemaProtocol::OpenAi);
    let anthropic_schema = definition
        .input_schema
        .for_protocol(ToolSchemaProtocol::Anthropic);

    assert_eq!(openai_schema, anthropic_schema);
    assert_eq!(openai_schema["type"], "object");
    assert_eq!(openai_schema["additionalProperties"], false);
    assert!(openai_schema["properties"]["path"].is_object());
    assert!(openai_schema["properties"]["start_line"].is_object());
    assert!(openai_schema["properties"]["end_line"].is_object());
}
