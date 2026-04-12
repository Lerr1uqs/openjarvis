
下面项目最终被添加进conversation的是下面四种, 一次输出其实会带推理，函数调用，和输出 实际上还有其他中 
```json
[
    {'role': 'user', 'content': '北京天气怎么样？'},
    ResponseReasoningItem( // response.output
        id='msg_db937da3-c872-4c8b-9fd5-c09d650a32db',
        summary=[Summary(text='用户询问北京的天气情况，需要调用 get_mock_weather 工具获取数据。\n参数：city 为“北京”，date 为 null 代表今天。', type='summary_text')],
        type='reasoning',
        content=None,
        encrypted_content=None,
        status=None
    ),
    ResponseFunctionToolCall(
        arguments='{"city": "北京", "date": "None"}',
        call_id='call_fc6b0cf712c94793adeb927d',
        name='get_mock_weather',
        type='function_call',
        id='msg_b487cc82-ce7c-418a-bbd0-419cdd5974e0',
        namespace=None,
        status='completed'
    ),
    {
        'type': 'function_call_output',
        'call_id': 'call_fc6b0cf712c94793adeb927d',
        'output': '{"city": "北京", "date": "2026-04-11", "condition": "Cloudy", "temperature_celsius": 15, "humidity_percent": 84, "wind_level": "Moderate breeze", "source": 
"mock-weather"}'
    }
]
```



```python
import json
import os
import sys
import uuid
from pathlib import Path
from typing import TypeAlias

from openai import OpenAI
from openai.types.responses.easy_input_message_param import EasyInputMessageParam
from openai.types.responses.response_input_param import (
    FunctionCallOutput,
    ResponseInputItemParam,
)
from openai.types.responses.response_output_item import ResponseOutputItem
from rich import print
from openai.types.responses.response import Response
from openai.types.responses.response_function_tool_call import ResponseFunctionToolCall

from mock_weather import OPENAI_WEATHER_TOOL, format_tool_result, get_mock_weather


DEFAULT_MODEL = os.getenv("OPENAI_MODEL", "qwen3.6-plus")
DEFAULT_BASE_URL = os.getenv(
    "DASHSCOPE_BASE_URL",
    "https://dashscope.aliyuncs.com/compatible-mode/v1",
)
DEFAULT_TOKEN_FILE = Path.home() / ".dashscope.apikey"
DEFAULT_PROMPT = "北京天气怎么样？"
SYSTEM_PROMPT = (
    "Use a short ReAct workflow. "
    "If the user asks for weather, call the provided tool instead of guessing. "
    "After receiving the tool result, answer briefly in Chinese."
)
# Keep prior model output items so the next call matches the official Responses API loop.
ConversationItem: TypeAlias = ResponseInputItemParam | ResponseOutputItem
Conversation: TypeAlias = list[ConversationItem]


def load_api_key() -> str:
    api_key = os.getenv("DASHSCOPE_API_KEY") or os.getenv("QWEN_API_KEY")
    if api_key:
        return api_key.strip()

    if DEFAULT_TOKEN_FILE.exists():
        return DEFAULT_TOKEN_FILE.read_text(encoding="utf-8").strip()

    raise RuntimeError(
        "Missing API key. Set DASHSCOPE_API_KEY/QWEN_API_KEY or create ~/.qwen_token."
    )


def build_extra_headers() -> dict[str, str]:
    headers = {
        "BCS-APIHub-RequestId": str(uuid.uuid4()),
    }

    gw_token = os.getenv("X_CHJ_GWTOKEN") or os.getenv("CHJ_GWTOKEN")
    if gw_token:
        headers["X-CHJ-GWToken"] = gw_token.strip()

    return headers


def build_prompt() -> str:
    if len(sys.argv) > 1:
        return " ".join(sys.argv[1:]).strip()
    return DEFAULT_PROMPT


def create_client() -> OpenAI:
    return OpenAI(
        base_url=DEFAULT_BASE_URL,
        api_key=load_api_key(),
    )


def build_user_message(prompt: str) -> EasyInputMessageParam:
    return {"role": "user", "content": prompt}

def build_function_call_output(call_id: str, output: str) -> FunctionCallOutput:
    return {
        "type": "function_call_output",
        "call_id": call_id,
        "output": output,
    }


def extract_function_calls(response: Response) -> list[ResponseFunctionToolCall]:
    return [
        item
        for item in response.output
        if getattr(item, "type", None) == "function_call"
    ]


def run_weather_tool(arguments_json: str) -> str:
    arguments = json.loads(arguments_json)
    result = get_mock_weather(
        city=arguments["city"],
        date=arguments.get("date"),
    )
    return format_tool_result(result)


def main() -> None:
    prompt = build_prompt()
    client = create_client()
    conversation: Conversation = [build_user_message(prompt)]

    print(f"[bold cyan]Question[/]: {prompt}")

    response = client.responses.create(
        model=DEFAULT_MODEL,
        instructions=SYSTEM_PROMPT,
        input=conversation,
        max_output_tokens=1024,
        max_tool_calls=4,
        parallel_tool_calls=False,
        tool_choice="auto",
        tools=[OPENAI_WEATHER_TOOL],
        extra_headers=build_extra_headers(),
    )

    while True:
        print(f"response: {response}")
        function_calls = extract_function_calls(response)
        if not function_calls:
            print(f"[bold green]Answer[/]: {response.output_text}")
            break

        conversation.extend(response.output)

        for call in function_calls:
            serialized_result = run_weather_tool(call.arguments)
            print(f"[yellow]Action[/]: {call.name}({call.arguments})")
            print(f"[magenta]Observation[/]: {serialized_result}")
            tool_output = build_function_call_output(call.call_id, serialized_result)
            print(f"tool_result: {tool_output}")
            conversation.append(tool_output)

        response = client.responses.create(
            model=DEFAULT_MODEL,
            instructions=SYSTEM_PROMPT,
            input=conversation,
            max_output_tokens=1024,
            max_tool_calls=4,
            parallel_tool_calls=False,
            tool_choice="auto",
            tools=[OPENAI_WEATHER_TOOL],
            extra_headers=build_extra_headers(),
        )
    
    print(conversation)


if __name__ == "__main__":
    main()

```