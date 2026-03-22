openai接口是原生支持toolcall的 应该去找openai的sdk而不是自己写parse

下面只是一个example 实际上你还是需要while true走react范式而不是两次调用
```rust
use async_openai::{
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs,
        CreateChatCompletionRequestArgs, Role, ChatCompletionTool,
        ChatCompletionToolType, FunctionObject,
    },
    Client,
};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::new();

    // 1️⃣ 定义 tool
    let tool = ChatCompletionTool {
        r#type: ChatCompletionToolType::Function,
        function: FunctionObject {
            name: "get_weather".to_string(),
            description: Some("Get weather by city".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            })),
        },
    };

    // 2️⃣ 用户输入
    let user_msg = ChatCompletionRequestMessageArgs::default()
        .role(Role::User)
        .content("北京天气怎么样？")
        .build()?;

    // 3️⃣ 第一次请求（让模型决定是否调用 tool）
    let req = CreateChatCompletionRequestArgs::default()
        .model("gpt-4.1")
        .messages([user_msg.clone()])
        .tools(vec![tool.clone()])
        .build()?;

    let resp = client.chat().create(req).await?;

    let msg = &resp.choices[0].message;

    // 4️⃣ 判断是否触发 tool call
    if let Some(tool_calls) = &msg.tool_calls {
        let call = &tool_calls[0];

        let func_name = &call.function.name;
        let args = &call.function.arguments;

        println!("👉 模型调用函数: {}", func_name);
        println!("👉 参数: {}", args);

        // 5️⃣ 执行你自己的函数（这里写死模拟）
        let result = if func_name == "get_weather" {
            // 解析参数
            let v: serde_json::Value = serde_json::from_str(args)?;
            let city = v["city"].as_str().unwrap_or("未知");

            format!("{}今天天气晴，25°C", city)
        } else {
            "unknown tool".to_string()
        };

        // 6️⃣ 把 tool 结果回传给模型
        let tool_msg = ChatCompletionRequestMessage::Tool(
            async_openai::types::ChatCompletionRequestToolMessageArgs::default()
                .tool_call_id(call.id.clone())
                .content(result)
                .build()?,
        );

        // ⚠️ 注意：要把 assistant 原始 message 也带回去
        let assistant_msg = ChatCompletionRequestMessage::Assistant(
            async_openai::types::ChatCompletionRequestAssistantMessageArgs::default()
                .tool_calls(tool_calls.clone())
                .build()?,
        );

        // 7️⃣ 第二次请求（让模型生成最终回答）
        let req2 = CreateChatCompletionRequestArgs::default()
            .model("gpt-4.1")
            .messages([user_msg, assistant_msg, tool_msg])
            .build()?;

        let final_resp = client.chat().create(req2).await?;

        println!(
            "\n✅ 最终回答:\n{}",
            final_resp.choices[0]
                .message
                .content
                .as_deref()
                .unwrap_or("")
        );
    }

    Ok(())
}
```