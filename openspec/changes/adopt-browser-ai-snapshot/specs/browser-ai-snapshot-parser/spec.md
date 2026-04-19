## ADDED Requirements

### Requirement: 系统 SHALL 提供 AI snapshot 到结构化 YAML 的独立解析脚本
系统 SHALL 提供一个独立 helper 脚本，用于读取 AI snapshot 文本并输出稳定的 YAML 结构。输出结构 MUST 至少保留节点的 `role`、可访问名称、`ref`、属性集合、属性节点、文本值与 `children` 层级，以便后续规则或脚本继续消费。

#### Scenario: 解析脚本把 AI snapshot 转成稳定 YAML 结构
- **WHEN** 调用方给脚本输入一份合法的 AI snapshot 文本
- **THEN** 脚本输出一份结构化 YAML，而不是仅原样回显文本
- **THEN** 输出中包含节点层级以及 `role`、`name`、`ref`、`attributes`、`props`、`value`、`children` 等稳定字段

### Requirement: 解析脚本 SHALL 汇总 AI snapshot 中出现的概念命名
解析脚本 SHALL 在输出中附带对当前 snapshot 概念命名的汇总，至少包括出现过的 role 名、属性名与属性节点名，便于后续规则、提取器或 diff 逻辑建立白名单与适配。

#### Scenario: 输出中包含概念汇总
- **WHEN** 一份 AI snapshot 被成功解析
- **THEN** 输出结果中包含 role 名集合
- **THEN** 输出结果中包含属性名集合与属性节点名集合

### Requirement: 解析脚本 SHALL 对不合法输入返回可定位的解析失败
当输入文本不是合法的 AI snapshot YAML、出现不可识别的层级结构，或节点 key 无法被解析时，脚本 SHALL 返回失败，并尽量附带可定位的错误信息，便于调用方快速定位问题。

#### Scenario: 非法 snapshot 输入会返回失败
- **WHEN** 调用方向解析脚本输入一份格式损坏或节点结构非法的 snapshot 文本
- **THEN** 脚本以失败状态退出
- **THEN** 错误输出中包含可用于定位问题的解析信息
