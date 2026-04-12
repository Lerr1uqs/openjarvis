
在线程初始化的时候，会根据当前配置文件中为这个 channel 指定的 features 去开启对应的功能，注入对应的 system prompt，并加载、激活对应的工具集。

比如 Memory、Skill、Auto Compactor 都是 feature

它不以一种固定的 rust trait 去进行注入，而是去枚举所有的 feature，再根据枚举的 feature 有没有被打开，来决定是否注入对应的功能。

# 验收标准

## 配置文件能否正常指定
主要是在单元测试中，根据不同的配置文件来验证解析的准确性与注入的正确性。

比如：当前这个 feature 开了 skill，那么在线程初始化之后，就必须要有 skill 的工具和对应的 system prompt。此外，如果遇到写错的 feature，系统能不能正常地拒绝并报出合适的错误？

关于 feature 的配置：
1. Feature 默认都是全开的。
2. 配置文件中填写的其实是 disabled features（即哪些被禁用）。
3. 可以通过控制配置文件，选择打开或关闭哪些 features，从而观察对应的注入情况是否符合预期。

## 持久化观测

features 是在 thread 这个模型的持久化层，也就是落盘的。

可以根据 thread 创造、落盘之后再重新加载出来，来观测：
1. 落盘实现和读出实现的完整性
2. 修改之后再落盘、再读出来的一致性

也就是说，要观测它持久化实现的正确性。
