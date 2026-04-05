# Active Memory 关键词必须按文件归并

## 问题

active memory catalog 之前错误地把每个关键词都展开成一条独立映射：

- `JJJ -> jjj_preference.md`
- `喜好 -> jjj_preference.md`
- `小南梁 -> jjj_preference.md`

这会把“一个文件有多个关键词”错误表达成“多个独立记忆条目”，让模型误以为它们是三条不同记忆。

## 正确规则

active memory catalog 必须以“文件”为主键：

- 一个文件一条 catalog entry
- 该文件的多个关键词按原顺序用英文逗号拼接
- 最终格式应为：`JJJ, 喜好, 小南梁 -> jjj_preference.md`

## 以后避免方式

实现 active memory prompt / catalog 时先问自己两个问题：

1. 当前结构的主键到底是“关键词”还是“文件”？
2. 如果一个文件声明 3 个关键词，最终输出会不会被错误展开成 3 条记录？

只要答案不是“按文件归并”，就说明实现方向错了，必须先修模型再写代码。
