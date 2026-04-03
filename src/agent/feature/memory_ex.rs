//! Memory 主要实现的功能是：用户提什么要求，系统能够主动往里面写一段记忆。
//! 
//! 记忆分为两类：
//! 
//! 1. 主动记忆（Active Memory）
//!    主要目标是当用户主动要求“把这个作为主动记忆”或“把这个记住”时，系统会记录下来。之后如果出现相关的关键词，系统就会主动召回这段记忆并注入上下文。
//!    (a) 前提是上下文还没有被注入过这段内容。
//!    (b) 如果已经注入过（变成 Message 了），就不需要再次注入。
//! 
//! 2. 被动记忆（Passive Memory）
//!    不需要主动注入，而是需要用户说“你去查一下这个东西”。系统会通过被动检索的方式，检索到对应记忆的文档。
//!    (a) 比如使用一些检索算法返回 Top 几个结果。
//!    (b) 再通过 Memory get 去获取这些结果。
//! 
//! 提供给agent的工具分为 以下四种
//! memory_get("2025-01-02/conversation.md") # 只支持md
//! memory_search("kw1,kw2")
//! memory_write("2025-01-02/perferences.md", content)
//! 
//! active_memory_write("kw1,kw2,..", "2025-01-02/perferences.md", content)

// Ex 代表实验
pub struct ActiveMemoryEx {

}

impl ActiveMemoryEx {
    fn load() -> Vec<ActiveMemoryItem> {
        // 从.openjarvis/ 加载本地的active memory
    }
    pub fn search(keywords: Vec<String>) -> Option<ChatMessage> {

        // 从本地的 ActiveMemoryItems中 寻找keywords匹配的 ActiveMemory 并concat为新的Prompt
        items = ActiveMemoryEx::load(...)

        // for item in items ...

        // amem = "以下是用户消息命中的 Active Memory"
        // amem += "memory item"

        // return ChatMessage(Asistant, amem)
    }
}

pub struct PassiveMemory {

}

impl PassiveMemory {
    // pub get
    // pub search 这里目前只是grep 以后可能有多重能力比如FTS5等等
    // pub write
}